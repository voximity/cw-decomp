//! The skills panel (X) and the quick bar's skill icons: the per-frame part of
//! `GameController::update` `Cube.exe 0x00488ee0` that places and paints the two
//! specialization buttons (`GC+0x800850/+0x800854`) and the eleven skill buttons
//! (`GC+0x800844`), `cube::SkillWidget::update` 0x004dd810 (its texts), the skill-panel clicks
//! of `onMouseDown` 0x0047b600, and the skill-bar icons and cooldown overlays of the six
//! quick-bar slots (`GC+0x8007fc`, cooldowns `GC+0x800808`).
//!
//! Tier B.
//!
//! # Map
//!
//! | Range | Here |
//! |---|---|
//! | ctor 0x0045e0f8..0x0046006e: the icon maps `+0x800820` / `+0x800828` / `+0x800830` | [`MODE_ICONS`], [`SKILL_ICONS`], [`SPEC_ICONS`] (loaded by `members::construct`) |
//! | ctor 0x004655a3..0x00465604: the eleven skill buttons | `members::construct` |
//! | `update` 0x0048b820..0x0048b8ab: every skill button hidden | [`panel_frame`] |
//! | `update` 0x0048bd8c..0x0048c100: the quick bar's icons and cooldowns | [`skill_bar_frame`] |
//! | `update` 0x0048da0b..0x0048da36: `PreviewWidget+0x168` / `+0x170` = -1 | [`panel_frame`] |
//! | `update` 0x0048da47..0x0048e1f6: the buttons' positions | [`button_positions`] |
//! | `update` 0x0048e1fa..0x0048e492: the specialization buttons | [`panel_frame`] |
//! | `update` 0x0048e498..0x0048ead2: the skill buttons | [`panel_frame`] |
//! | `update` 0x0048ead4..0x0048eaef: panel closed | [`panel_frame`] |
//! | `update` 0x0048eaef..0x0048ec93: the menu buttons' hover brightness | [`menu_bar_frame`] |
//! | `SkillWidget::update` 0x004dd810 | [`widget_texts`] |
//! | `onMouseDown` 0x0047b849..0x0047baab | [`on_mouse_down`] |
//! | `onMouseDown` 0x0047c806..0x0047c8ea (Learn / Cancel) | [`learn_or_cancel`] |

use cw_net::EntityData;
use cw_ui::widget::{Gui, NodeId, WidgetId};
use glam::Vec2;

use super::crafting::PanelText;
use super::members::{find_node, set_shape_attr_f32, set_shape_texture};
use super::present_hud::{place, set_all_text};
use super::{GameUi, GameView, UiAction};

/// `GC+0x800820` (ctor 0x0045e12d..0x0045f9..): attack mode → icon file of `data3.db`.
pub const MODE_ICONS: &[(i32, &str)] = &[
    (0x0, "none.png"),
    (0x39, "slam.png"),
    (0x3a, "slam-back.png"),
    (0x41, "slam-left.png"),
    (0x42, "slam-right.png"),
    (0x2, "attack-left.png"),
    (0x1, "attack-right.png"),
    (0x4, "stab-left.png"),
    (0x3, "stab-right.png"),
    (0x43, "jab.png"),
    (0x3b, "charge-slam.png"),
    (0x3f, "swirl.png"),
    (0x40, "charge-mutilate.png"),
    (0x8, "block.png"),
    (0x9, "shield-attack.png"),
    (0xa, "shield-slam.png"),
    (0x12, "pierce-left.png"),
    (0x13, "pierce-right.png"),
    (0x11, "ambush.png"),
    (0x7, "punch-left.png"),
    (0x6, "punch-right.png"),
    (0x14, "kick.png"),
    (0x15, "ranger-kick.png"),
    (0xd, "slash-left.png"),
    (0xe, "slash-right.png"),
    (0x5, "perforate.png"),
    (0x4f, "sneak.png"),
    (0x63, "aim.png"),
    (0x64, "swiftness.png"),
    (0x26, "firebolt.png"),
    (0x27, "firebolt.png"),
    (0x28, "firebolt.png"),
    (0x25, "fireball.png"),
    (0x2e, "fire-salvo.png"),
    (0x58, "fire-explosion.png"),
    (0x2c, "waterbolt.png"),
    (0x29, "waterbolt.png"),
    (0x2a, "waterbolt.png"),
    (0x2b, "waterball.png"),
    (0x2d, "water-salvo.png"),
    (0x22, "healing-stream.png"),
    (0x1e, "fire-shock.png"),
    (0x1f, "fire-burst.png"),
    (0x20, "water-shock.png"),
    (0x21, "water-burst.png"),
    (0x36, "smash.png"),
    (0xb, "swirl.png"),
    (0x56, "cyclone.png"),
    (0x30, "intercept.png"),
    (0x31, "teleport.png"),
    (0x32, "retreat2.png"),
    (0xc, "slash.png"),
    (0x10, "mutilate.png"),
    (0x16, "shoot.png"),
    (0x17, "shoot-fast.png"),
    (0x37, "salvo.png"),
    (0x1a, "boomerang.png"),
    (0x1b, "charge-boomerang.png"),
    (0x18, "charge-shoot.png"),
    (0x19, "salvo.png"),
    (0x1c, "beam.png"),
    (0x5f, "beam.png"),
    (0x24, "charge-bullet.png"),
    (0x60, "shuriken.png"),
    (0x65, "bulwark.png"),
    (0x67, "mana-shield.png"),
    (0x61, "camouflage.png"),
    (0x66, "warfrenzy.png"),
];

/// `GC+0x800828` (ctor 0x0045f9..0x0045fb62): skill index → icon (the non-class skills).
pub const SKILL_ICONS: &[(i32, &str)] = &[
    (1, "pet-riding.png"),
    (0, "pet-taming.png"),
    (3, "hang-gliding.png"),
    (2, "climbing.png"),
    (4, "swimming.png"),
    (5, "ship-driving.png"),
];

/// `GC+0x800830` (ctor 0x0045fbda..0x0046001c, keys built by 0x00457ea0): (class,
/// specialization) → icon.
pub const SPEC_ICONS: &[((u8, i32), &str)] = &[
    ((3, 0), "mage-fire.png"),
    ((3, 1), "mage-water.png"),
    ((1, 0), "warrior-berserker.png"),
    ((1, 1), "warrior-guardian.png"),
    ((2, 0), "ranger-sniper.png"),
    ((2, 1), "ranger-scout.png"),
    ((4, 1), "rogue-ninja.png"),
    ((4, 0), "rogue-assassin.png"),
];

const WHITE: [f32; 4] = [1.0; 4];
const CYAN: [f32; 4] = [0.0, 1.0, 1.0, 1.0];

/// What the skill buttons asked of the `PreviewWidget` (`GC+0x8008e8`) this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SkillHover {
    /// A button is hovered: `preview` shown and `[esp+0x17]` (the near-cursor placement) set.
    pub hovered: bool,
    /// `Preview+0x168`: the hovered skill (-1 none).
    pub skill: i32,
    /// `Preview+0x16c`: its level in the widget's allocation.
    pub level: i32,
    /// `Preview+0x170`: the hovered specialization (-1 none).
    pub spec: i32,
}

/// One quick-bar slot's skill as `update` 0x0048bd8c reads it: the mode in the skill bar
/// (`GC+0x800814[i]`), its running cooldown (`creature+0x139c` map, 0 when absent), the full
/// cooldown `skillCooldown(creature, mode, -1)` 0x0043e6a0 and `canUseSkill` 0x0043e5a0.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SkillBarSlot {
    /// The attack mode (0: no points in the skill).
    pub mode: i32,
    /// The running cooldown in ms.
    pub cooldown: i32,
    /// `skillCooldown(creature, mode, -1)`.
    pub total: i32,
    /// `0x0043e5a0` (`cw_sim::combat_ai::can_use`).
    pub usable: bool,
}

/// The positions 0x0048da47..0x0048e1f6 give the buttons (before the pivot is subtracted,
/// `place`): from `x0 = (int)(panel.x + 50)`, `y0 = (int)(panel.y + 70)` (the panel's widget
/// position, 0x0062f630 / 0x0062f660, `cvttss2si`): the specializations at `(x0, y0)` and
/// `(x0 + 70, y0)`; the class skills 6..9 on the row `y0 + 80` at `x0`, `+55`, `+110`, `+165`;
/// then the pairs (0, 1), (2, 3), (4, 5) at `x0` / `x0 + 55` on the rows `y0 + 140`, `+200`,
/// `+260`. Skill 10 is not placed (it is never shown).
pub fn button_positions(panel: Vec2) -> ([[f32; 2]; 2], [Option<[f32; 2]>; 11]) {
    let x0 = super::inventory::cvtt(panel.x + 50.0f32);
    let y0 = super::inventory::cvtt(panel.y + 70.0f32);
    let p = |x: i32, y: i32| Some([x as f32, y as f32]);
    let spec = [[x0 as f32, y0 as f32], [(x0 + 0x46) as f32, y0 as f32]];
    let y1 = y0 + 0x50;
    let mut s = [None; 11];
    s[6] = p(x0, y1);
    s[7] = p(x0 + 0x37, y1);
    s[8] = p(x0 + 0x6e, y1);
    s[9] = p(x0 + 0xa5, y1);
    s[0] = p(x0, y1 + 0x3c);
    s[1] = p(x0 + 0x37, y1 + 0x3c);
    s[2] = p(x0, y1 + 0x78);
    s[3] = p(x0 + 0x37, y1 + 0x78);
    s[4] = p(x0, y1 + 0xb4);
    s[5] = p(x0 + 0x37, y1 + 0xb4);
    (spec, s)
}

/// The class skill of button 6, 7 or 8 (0x0048e751.., jump tables 0x0049d3b0 / 0x0049d3a0 /
/// 0x0049d390 over `class - 1`), looked up in `GC+0x800820`; `None` for another button or class.
pub fn class_skill_mode(button: usize, class: u8, spec: i32) -> Option<i32> {
    match (button, class) {
        (6, 1) => Some(0x36),
        (6, 2) => Some(0x15),
        (6, 3) => Some(if spec == 1 { 0x22 } else { 0x58 }),
        (6, 4) => Some(0x30),
        (7, 1) => Some(0x56),
        (7, 2) => Some(0x32),
        (7, 3) => Some(0x67),
        (7, 4) => Some(0x4f),
        (8, 1) => Some(if spec == 0 { 0x66 } else { 0x65 }),
        (8, 2) => Some(if spec == 1 { 0x64 } else { 0x63 }),
        (8, 3) => Some(0x31),
        (8, 4) => Some(if spec == 1 { 0x60 } else { 0x61 }),
        _ => None,
    }
}

/// The child skill locks of 0x0048e646..0x0048e6e3: 7, 8, 9 need 6, 7, 8 at 5; 1, 3, 5 need
/// 0, 2, 4 at 5 (the icon is then drawn grey, saturation 0).
fn locked(skills: &[i32; 11], i: usize) -> bool {
    match i {
        7 => skills[6] < 5,
        8 => skills[7] < 5,
        9 => skills[8] < 5,
        1 => skills[0] < 5,
        3 => skills[2] < 5,
        5 => skills[4] < 5,
        _ => false,
    }
}

fn widget_of(gui: &Gui, n: NodeId) -> Option<WidgetId> {
    gui.nodes[n].widget
}

/// The `frame` child's Display fill colour (0x00633d70 "frame", 0x0040f8e0, `+0xf8`
/// 0x004288e0), for `present_panels::node_colors`.
fn frame_color(gui: &Gui, n: NodeId, c: [f32; 4], out: &mut Vec<(NodeId, [f32; 4])>) {
    if let Some(f) = find_node(gui, n, "frame") {
        out.push((f, c));
    }
}

/// The skills-panel part of `update` 0x0048b820.. and 0x0048da0b..0x0048eaef: every skill
/// button hidden, the preview's skill and specialization reset; with the panel open
/// (`GC+0x8008f1`) the buttons placed ([`button_positions`]) and shown, their `frame`
/// coloured cyan (0, 1, 1, 1) when hovered (the hovered one fills the preview) or white, the
/// specialization icon of `(class, i)` with saturation 1 for the chosen one and 0 otherwise
/// (texture -1 without an icon), and per skill button its level written as text over the whole
/// button (`Node::setText` 0x00636ad0), the icon (`GC+0x800820` by the class skill of buttons
/// 6..8, `GC+0x800828` by index otherwise, -1 without one) with saturation 0 when locked and 1
/// otherwise. Closed, both specialization buttons (and skill 9 again) are hidden.
pub fn panel_frame(ui: &mut GameUi, game: &GameView, colors: &mut Vec<(NodeId, [f32; 4])>) -> SkillHover {
    let mut h = SkillHover { hovered: false, skill: -1, level: 0, spec: -1 };
    let m = ui.m.clone();
    // 0x0048b820: every skill button hidden.
    for &n in &m.skill_buttons {
        ui.gui.nodes[n].visible = false;
    }
    if !ui.skills_open {
        // 0x0048ead4..0x0048eaef.
        for n in m.spec_buttons.iter().flatten() {
            ui.gui.nodes[*n].visible = false;
        }
        if let Some(&n) = m.skill_buttons.get(9) {
            ui.gui.nodes[n].visible = false;
        }
        return h;
    }
    let panel_pos = m.skills_panel.and_then(|n| widget_of(&ui.gui, n)).map_or(Vec2::ZERO, |w| ui.gui.get_position(w));
    let (spec_pos, skill_pos) = button_positions(panel_pos);
    for (i, n) in m.spec_buttons.iter().enumerate() {
        if let Some(n) = *n {
            place(&mut ui.gui, n, spec_pos[i]);
        }
    }
    for (i, &n) in m.skill_buttons.iter().enumerate() {
        if let Some(p) = skill_pos.get(i).copied().flatten() {
            place(&mut ui.gui, n, p);
        }
    }
    let class = game.player.0[0x130];
    // 0x0048e1fa..0x0048e492: the specialization buttons.
    for (i, n) in m.spec_buttons.iter().enumerate() {
        let Some(n) = *n else { continue };
        ui.gui.nodes[n].visible = true;
        let hovered = widget_of(&ui.gui, n).is_some_and(|w| ui.is_hovered(w));
        if hovered {
            frame_color(&ui.gui, n, CYAN, colors);
            h.spec = i as i32;
            h.hovered = true;
        } else {
            frame_color(&ui.gui, n, WHITE, colors);
        }
        match m.spec_icons.get(&(class, i as i32)) {
            Some(&t) => {
                set_shape_texture(&mut ui.gui, n, t);
                let sat = if i as i32 == ui.skills.spec { 1.0 } else { 0.0 };
                set_shape_attr_f32(&mut ui.gui, n, ShapeAttr::TextureSaturation, sat);
            }
            None => set_shape_texture(&mut ui.gui, n, -1),
        }
    }
    // 0x0048e498..0x0048ead2: the skill buttons.
    for (i, &n) in m.skill_buttons.iter().enumerate() {
        ui.gui.nodes[n].visible = i != 10;
        let level = ui.skills.skills[i];
        let hovered = widget_of(&ui.gui, n).is_some_and(|w| ui.is_hovered(w));
        if hovered {
            frame_color(&ui.gui, n, CYAN, colors);
            h.skill = i as i32;
            h.level = level;
            h.hovered = true;
        } else {
            frame_color(&ui.gui, n, WHITE, colors);
        }
        let lock = locked(&ui.skills.skills, i);
        // `wostringstream << level` then `Node::setText`.
        set_all_text(&mut ui.gui, n, &level.to_string());
        let icon = match i {
            6..=8 => class_skill_mode(i, class, ui.skills.spec).and_then(|mode| m.mode_icons.get(&mode).copied()),
            _ => m.skill_icons.get(&(i as i32)).copied(),
        };
        match icon {
            Some(t) => {
                set_shape_texture(&mut ui.gui, n, t);
                set_shape_attr_f32(&mut ui.gui, n, ShapeAttr::TextureSaturation, if lock { 0.0 } else { 1.0 });
            }
            None => set_shape_texture(&mut ui.gui, n, -1),
        }
    }
    h
}

pub use super::members::ShapeAttr;

/// `update` 0x0048eaef..0x0048ec93: the menu buttons (`GC+0x800838`) shown with the system
/// menu bar (`GC+0x8008f2`, also done by `flow::frame_rules`); while it is up, each button's
/// texture brightness (`+0x494`) is 1.5 when hovered and 1 otherwise.
pub fn menu_bar_frame(ui: &mut GameUi) {
    let m = ui.m.clone();
    for &n in &m.menu_buttons {
        ui.gui.nodes[n].visible = ui.system_menu;
    }
    if !ui.system_menu {
        return;
    }
    for &n in &m.menu_buttons {
        let hovered = menu_button_hovered(ui, n);
        set_shape_attr_f32(&mut ui.gui, n, ShapeAttr::TextureBrightness, if hovered { 1.5 } else { 1.0 });
    }
}

/// `Button::isHovered` 0x006294c0 of a menu button (`Widget::isUnderCursor` 0x00629300 on its
/// node): the node under the cursor is the button's node or one of its children (the icon
/// and the key caption of the `menubutton` clone).
fn menu_button_hovered(ui: &GameUi, n: NodeId) -> bool {
    widget_of(&ui.gui, n).is_some_and(|w| ui.is_hovered(w)) || ui.gui.hovered.is_some_and(|h| ui.gui.is_ancestor_or_self(n, Some(h)))
}

/// The menu button `onMouseDown` 0x0047b7a5 finds hovered (index into
/// `GcMembers::menu_buttons`), by the same test as the highlight of [`menu_bar_frame`].
pub fn hovered_menu_button(ui: &GameUi) -> Option<usize> {
    ui.m.menu_buttons.iter().position(|&n| menu_button_hovered(ui, n))
}

/// `update` 0x0048bd8c..0x0048c100: for each quick-bar slot `i` (the six ability buttons and
/// the quick-item button): with `i` inside the skill bar, the cooldown node of the first six
/// is shown with the scale `(1, max(cooldown / total, 0))` (the identity deformation 0x00423e70
/// then 0x00434ad0) while the mode has a running cooldown and a non-zero full cooldown, else
/// hidden; the icon is `GC+0x800820[mode]` with saturation 1 when the skill can be used
/// (`0x0043e5a0`) and 0 otherwise. Outside the bar, or without an icon for the mode, the icon
/// is `GC+0x800820[0]` (`none.png`), saturation untouched.
pub fn skill_bar_frame(ui: &mut GameUi, bar: &[SkillBarSlot]) {
    let m = ui.m.clone();
    for (i, &slot) in m.quick_slots.iter().enumerate() {
        let mut icon = None;
        if let Some(s) = bar.get(i) {
            if let Some(&c) = m.quick_cooldowns.get(i) {
                if s.cooldown != 0 && s.total != 0 {
                    ui.gui.nodes[c].visible = true;
                    let f = s.cooldown as f32 / s.total as f32;
                    // `comiss 0, f; jbe`: a negative fill becomes 0 (NaN passes through).
                    let f = if 0.0 > f { 0.0 } else { f };
                    let mut d = glam::Mat4::IDENTITY.to_cols_array();
                    d[5] = f;
                    ui.gui.nodes[c].deformation = d;
                    ui.gui.nodes[c].deform_dirty = true;
                } else {
                    ui.gui.nodes[c].visible = false;
                }
            }
            if let Some(&t) = m.mode_icons.get(&s.mode) {
                icon = Some(t);
                set_shape_texture(&mut ui.gui, slot, t);
                set_shape_attr_f32(&mut ui.gui, slot, ShapeAttr::TextureSaturation, if s.usable { 1.0 } else { 0.0 });
            }
        }
        if icon.is_none() {
            if let Some(&t) = m.mode_icons.get(&0) {
                set_shape_texture(&mut ui.gui, slot, t);
            }
        }
    }
}

/// `SkillWidget::update` 0x004dd810 (widget-local, font `resource1.dat`, each text an outline
/// pass then a fill pass): "Skills" at (15, 25) size 12; "Points: spent/(level * 2 - 2)" at
/// (15, 45) size 10 (the displayed cap has no mana-cube share, unlike the click test); with a
/// cost (0x004df9c0) above 0, at `y = (int)(h - 65)`: "Not enough money." (or, affordable but
/// with no class trainer near (0x0047f030), "Requires class trainer.") in (1, 0.25, 0.25, 1),
/// then at `y + 20` "COST:" and the copper / silver / gold amounts right-aligned at x 220 /
/// 180 / 140; with the allocation changed, "Learn" centred at `(w / 3, h - 20)` (grey 0.5
/// when unaffordable or without a trainer, cyan when [`super::skills::SkillWidget::learn_hit`]
/// holds, else white) and "Cancel" at `(2w / 3, h - 20)` (cyan when `cancel_hit`).
pub fn widget_texts(ui: &GameUi, game: &GameView, cursor: Vec2, size: Vec2) -> Vec<PanelText> {
    let w = &ui.skills;
    let p = &game.player;
    let mut out = Vec::new();
    out.push(PanelText::new("Skills", 15.0, 25.0, 12.0, 3.0, WHITE, 0));
    let spent: i32 = w.skills.iter().sum();
    let level = i32::from_le_bytes(p.0[0x180..0x184].try_into().unwrap());
    let cap = level * 2 - 2;
    out.push(PanelText::new(format!("Points: {spent}/{cap}"), 15.0, 45.0, 10.0, 2.0, WHITE, 0));
    let changed = (0..11).any(|k| w.skills[k] != i32::from_le_bytes(p.0[0x1128 + 4 * k..0x112c + 4 * k].try_into().unwrap()))
        || w.spec != i32::from(p.0[0x131]);
    let cost = w.cost(p);
    let mut affordable = cost <= game.coins;
    if 0 < cost {
        let (gold, silver) = ((cost / 100) / 100, (cost / 100) % 100);
        let mut y = super::inventory::cvtt(size.y - 65.0f32);
        let red = [1.0, 0.25, 0.25, 1.0];
        if !affordable {
            out.push(PanelText::new("Not enough money.", 15.0, y as f32, 10.0, 2.0, red, 0));
        } else if !game.trainer_nearby {
            out.push(PanelText::new("Requires class trainer.", 15.0, y as f32, 10.0, 2.0, red, 0));
            affordable = false;
        }
        y += 0x14;
        let y = y as f32;
        out.push(PanelText::new(format!("{} C ", cost % 100), 220.0, y, 10.0, 2.0, [0.8, 0.5, 0.0, 1.0], 2));
        out.push(PanelText::new(format!("{silver} S "), 180.0, y, 10.0, 2.0, [0.7, 0.7, 0.7, 1.0], 2));
        out.push(PanelText::new(format!("{gold} G "), 140.0, y, 10.0, 2.0, [1.0, 0.9, 0.0, 1.0], 2));
        out.push(PanelText::new("COST:", 15.0, y, 10.0, 2.0, WHITE, 0));
    }
    if changed {
        let learn = if !affordable {
            [0.5, 0.5, 0.5, 1.0]
        } else if w.learn_hit(p, game.coins, game.trainer_nearby, cursor, size) {
            CYAN
        } else {
            WHITE
        };
        let y = size.y - 20.0;
        out.push(PanelText::new("Learn", size.x / 3.0, y, 12.0, 3.0, learn, 1));
        let cancel = if w.cancel_hit(p, cursor, size) { CYAN } else { WHITE };
        out.push(PanelText::new("Cancel", (size.x * 2.0) / 3.0, y, 12.0, 3.0, cancel, 1));
    }
    out
}

/// `onMouseDown` 0x0047b849..0x0047baab, with the skills panel open: the left button on a
/// hovered specialization button selects it (sound 0x55); then any button on a hovered skill
/// button raises it (left, [`super::skills::SkillWidget::raise`]) or lowers it (other
/// buttons, `lower`).
pub fn on_mouse_down(ui: &mut GameUi, game: &GameView, button: i32) -> Vec<UiAction> {
    let mut out = Vec::new();
    if !ui.skills_open {
        return out;
    }
    let m = ui.m.clone();
    if button == 0 {
        for (i, n) in m.spec_buttons.iter().enumerate() {
            if n.and_then(|n| widget_of(&ui.gui, n)).is_some_and(|w| ui.is_hovered(w)) {
                out.push(ui.skills.set_spec(i as i32));
            }
        }
    }
    for (i, &n) in m.skill_buttons.iter().enumerate() {
        if !widget_of(&ui.gui, n).is_some_and(|w| ui.is_hovered(w)) {
            continue;
        }
        let a = if button == 0 { ui.skills.raise(i, &game.player) } else { ui.skills.lower(i) };
        out.extend(a);
    }
    out
}

/// `onMouseDown` 0x0047c806..0x0047c8ea (left button, skills panel open): Learn
/// (0x004df880) pays the cost from the coins when affordable and writes the allocation and
/// the specialization back to the player; then Cancel (0x004df760) copies the player's skills
/// and specialization back into the widget.
pub fn learn_or_cancel(ui: &mut GameUi, game: &mut GameView, cursor: Vec2, size: Vec2) {
    if !ui.skills_open {
        return;
    }
    let player: &EntityData = &game.player;
    if ui.skills.learn_hit(player, game.coins, game.trainer_nearby, cursor, size) && ui.skills.cost(player) <= game.coins {
        let w = ui.skills;
        w.learn(&mut game.player, &mut game.coins);
    }
    if ui.skills.cancel_hit(&game.player, cursor, size) {
        let p = game.player.clone();
        ui.skills.load_from(&p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The click finds a menu button when the node under the cursor is one of its children
    /// (the caption), as the highlight does.
    #[test]
    fn menu_button_hovered_through_a_child() {
        use super::super::members::NoPlx;
        let game = GameView::default();
        let mut ui = GameUi::new(Gui::new(), &mut NoPlx, &game);
        let n = ui.m.menu_buttons[5];
        let child = ui.gui.add_plain_node(Some(n), "caption");
        ui.gui.hovered = Some(child);
        assert_eq!(hovered_menu_button(&ui), Some(5));
        ui.gui.hovered = Some(ui.m.menu_buttons[2]);
        assert_eq!(hovered_menu_button(&ui), Some(2));
        ui.gui.hovered = None;
        assert_eq!(hovered_menu_button(&ui), None);
    }

    #[test]
    fn layout_of_the_buttons() {
        let (spec, s) = button_positions(Vec2::new(770.0, 403.0));
        assert_eq!(spec, [[820.0, 473.0], [890.0, 473.0]]);
        assert_eq!(s[6], Some([820.0, 553.0]));
        assert_eq!(s[9], Some([985.0, 553.0]));
        assert_eq!(s[1], Some([875.0, 613.0]));
        assert_eq!(s[4], Some([820.0, 733.0]));
        assert_eq!(s[10], None);
    }

    #[test]
    fn class_skills() {
        assert_eq!(class_skill_mode(6, 3, 1), Some(0x22));
        assert_eq!(class_skill_mode(8, 4, 0), Some(0x61));
        assert_eq!(class_skill_mode(5, 1, 0), None);
        assert!(locked(&[0; 11], 7));
        assert!(!locked(&[0; 11], 6));
    }

    #[test]
    fn skill_bar_cooldown_scale() {
        use cw_ui::widget::NodeSource;
        let mut ui = GameUi::default();
        for i in 0..2 {
            let n = ui.gui.add_plain_node(None, "abilitybutton");
            let c = ui.gui.add_node(Some(n), NodeSource { name: "cooldown".into(), visible: false, ..Default::default() });
            ui.m.quick_slots.push(n);
            ui.m.quick_cooldowns.push(c);
            let _ = i;
        }
        let bar = [
            SkillBarSlot { mode: 0x36, cooldown: 5000, total: 20000, usable: false },
            SkillBarSlot { mode: 0x56, cooldown: 0, total: 40000, usable: true },
        ];
        skill_bar_frame(&mut ui, &bar);
        let (c0, c1) = (ui.m.quick_cooldowns[0], ui.m.quick_cooldowns[1]);
        assert!(ui.gui.nodes[c0].visible);
        assert_eq!(ui.gui.nodes[c0].deformation[5], 0.25);
        assert_eq!(ui.gui.nodes[c0].deformation[0], 1.0);
        assert!(!ui.gui.nodes[c1].visible);
    }

    #[test]
    fn widget_texts_points_and_cost() {
        let mut ui = GameUi::default();
        let mut game = GameView::default();
        game.player.0[0x180..0x184].copy_from_slice(&5i32.to_le_bytes());
        game.player.0[0x1128..0x112c].copy_from_slice(&2i32.to_le_bytes());
        ui.skills.load_from(&game.player);
        let t = widget_texts(&ui, &game, Vec2::ZERO, Vec2::new(300.0, 460.0));
        assert_eq!(t[1].text, "Points: 2/8");
        assert_eq!(t.len(), 2);
        // Lowering a learnt skill costs money: the cost block and the buttons appear.
        ui.skills.lower(0);
        let t = widget_texts(&ui, &game, Vec2::ZERO, Vec2::new(300.0, 460.0));
        assert!(t.iter().any(|x| x.text == "Not enough money."));
        assert!(t.iter().any(|x| x.text == "Learn" && x.color == [0.5, 0.5, 0.5, 1.0]));
        assert!(t.iter().any(|x| x.text == "COST:" && x.y == 415.0));
    }
}
