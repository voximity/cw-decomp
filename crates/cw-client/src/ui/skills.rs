//! `cube::SkillWidget` (vtable 0x00703a34, ctor 0x004dd750, 0x194 bytes): the skill tree
//! the player spends points in (X). The widget edits a copy of the player's skills; "Learn"
//! writes it back.
//!
//! | Field | Offset |
//! |---|---|
//! | 11 skill levels (copy of `creature+0x1138..`) | +0x160..+0x188 |
//! | specialization (copy of `creature+0x141`) | +0x18c |
//! | GameController* | +0x190 |
//!
//! The clicks are handled by `GameController::onMouseDown` 0x0047b600 while `GC+0x8008f1` is
//! set; the Learn/Cancel tests are 0x004df880 / 0x004df760, the cost 0x004df9c0.

use cw_net::EntityData;
use glam::Vec2;

use super::UiAction;

/// The widget's state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SkillWidget {
    /// +0x160..+0x188.
    pub skills: [i32; 11],
    /// +0x18c.
    pub spec: i32,
}

fn i32_at(e: &EntityData, o: usize) -> i32 {
    i32::from_le_bytes(e.0[o..o + 4].try_into().unwrap())
}

/// The skill whose level must reach 5 before `i` can be raised (and which blocks lowering
/// its parent below 6): pairs (0, 1), (2, 3), (4, 5), (6, 7), (7, 8), (8, 9).
fn parent_of(i: usize) -> Option<usize> {
    match i {
        1 => Some(0),
        3 => Some(2),
        5 => Some(4),
        7 => Some(6),
        8 => Some(7),
        9 => Some(8),
        _ => None,
    }
}

fn child_of(i: usize) -> Option<usize> {
    match i {
        0 => Some(1),
        2 => Some(3),
        4 => Some(5),
        6 => Some(7),
        7 => Some(8),
        8 => Some(9),
        _ => None,
    }
}

impl SkillWidget {
    /// 0x00488c70 (opening) and the Cancel path: copy the player's skills
    /// (`entity+0x1128`, `creature+0x1138`) and specialization (`entity+0x131`).
    pub fn load_from(&mut self, player: &EntityData) {
        for k in 0..11 {
            self.skills[k] = i32_at(player, 0x1128 + 4 * k);
        }
        self.spec = i32::from(player.0[0x131]);
    }

    /// The points available: `manaCubes / 4 + level * 2 - 2` (`entity+0x1154`, `+0x180`;
    /// signed division toward zero).
    pub fn cap(player: &EntityData) -> i32 {
        let cubes = i32_at(player, 0x1154);
        cubes / 4 + i32_at(player, 0x180) * 2 - 2
    }

    /// Left click on skill button `i` (0x0047b8c5..): +1 when fewer points are spent than
    /// the cap and the parent skill is at 5 or more; plays 0x55.
    pub fn raise(&mut self, i: usize, player: &EntityData) -> Option<UiAction> {
        let total: i32 = self.skills.iter().sum();
        if !(total < Self::cap(player)) {
            return None;
        }
        if let Some(p) = parent_of(i) {
            if self.skills[p] < 5 {
                return None;
            }
        }
        self.skills[i] += 1;
        Some(UiAction::PlaySound { id: 0x55, volume: 1.0, pitch: 1.0 })
    }

    /// Right click on skill button `i`: -1 (floored at 0) unless the level is below 6 and the
    /// child skill has points; plays 0x58.
    pub fn lower(&mut self, i: usize) -> Option<UiAction> {
        let v = self.skills[i];
        if v < 6 {
            if let Some(c) = child_of(i) {
                if 0 < self.skills[c] {
                    return None;
                }
            }
        }
        self.skills[i] = v - 1;
        if self.skills[i] < 0 {
            self.skills[i] = 0;
        }
        Some(UiAction::PlaySound { id: 0x58, volume: 1.0, pitch: 1.0 })
    }

    /// The specialization radio (the two `specializationbutton` clones): `spec = i`, 0x55.
    pub fn set_spec(&mut self, i: i32) -> UiAction {
        self.spec = i;
        UiAction::PlaySound { id: 0x55, volume: 1.0, pitch: 1.0 }
    }

    fn changed(&self, player: &EntityData) -> bool {
        (0..11).any(|k| self.skills[k] != i32_at(player, 0x1128 + 4 * k)) || self.spec != i32::from(player.0[0x131])
    }

    /// 0x004df9c0: lowering a skill costs `itemPower(level) * (old - new) * 10` per skill,
    /// changing the specialization `itemPower(level) * 100` when the level is above 1; raising
    /// is free. Accumulated in single precision, returned truncated (`cvttss2si`).
    pub fn cost(&self, player: &EntityData) -> i32 {
        let level = i32_at(player, 0x180);
        let mut acc = 0.0f32;
        for k in 0..11 {
            let cur = i32_at(player, 0x1128 + 4 * k);
            if self.skills[k] < cur {
                let p = cw_sim::stats::item_power(level as f32, 0);
                acc = p * ((cur - self.skills[k]) as f32) * 10.0f32 + acc;
            }
        }
        if self.spec != i32::from(player.0[0x131]) && level > 1 {
            acc = cw_sim::stats::item_power(level as f32, 0) * 100.0f32 + acc;
        }
        acc as i32
    }

    /// 0x004df880: the Learn button is "pressed" when the allocation changed, its cost is
    /// affordable, a non-zero cost has a class trainer nearby (0x0047f030, `trainer`), and the
    /// local cursor is in the bottom-left band (`h - 30 < y < h`, `0 < x < w / 2`).
    pub fn learn_hit(&self, player: &EntityData, coins: i32, trainer: bool, cursor: Vec2, size: Vec2) -> bool {
        if !self.changed(player) {
            return false;
        }
        let c = self.cost(player);
        if !(c <= coins) {
            return false;
        }
        if 0 < c && !trainer {
            return false;
        }
        size.y - 30.0 < cursor.y && cursor.y < size.y && cursor.x < size.x * 0.5 && 0.0 < cursor.x
    }

    /// 0x004df760: Cancel, the bottom-right band (`w / 2 < x < w`).
    pub fn cancel_hit(&self, player: &EntityData, cursor: Vec2, size: Vec2) -> bool {
        if !self.changed(player) {
            return false;
        }
        size.y - 30.0 < cursor.y && cursor.y < size.y && size.x * 0.5 < cursor.x && cursor.x < size.x
    }

    /// The Learn click (0x0047cb.. in `onMouseDown`): pay the cost and write the allocation
    /// back to the player.
    pub fn learn(&self, player: &mut EntityData, coins: &mut i32) {
        let c = self.cost(player);
        if c <= *coins {
            *coins -= c;
            for k in 0..11 {
                player.0[0x1128 + 4 * k..0x112c + 4 * k].copy_from_slice(&self.skills[k].to_le_bytes());
            }
            player.0[0x131] = self.spec as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocation_rules() {
        let mut p = EntityData::new_creature();
        p.0[0x180..0x184].copy_from_slice(&5i32.to_le_bytes()); // level 5: cap 8
        let mut w = SkillWidget::default();
        w.load_from(&p);
        assert_eq!(SkillWidget::cap(&p), 8);
        assert!(w.raise(1, &p).is_none()); // needs skill 0 at 5
        for _ in 0..5 {
            assert!(w.raise(0, &p).is_some());
        }
        assert!(w.raise(1, &p).is_some());
        assert!(w.lower(0).is_none()); // 5 < 6 and the child has a point
        for _ in 0..2 {
            w.raise(2, &p);
        }
        assert!(w.raise(4, &p).is_none()); // 8 points spent
        assert_eq!(w.cost(&p), 0); // raising is free
        let mut coins = 0;
        w.learn(&mut p, &mut coins);
        assert_eq!(i32_at(&p, 0x1128), 5);
        // Lowering an already learnt skill costs money.
        w.lower(2);
        assert!(w.cost(&p) > 0);
    }
}
