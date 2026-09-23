//! The quick item (the consumable Q uses): the quick-item button's stack count on the HUD
//! and the Tab wheel of 3D item models `render` draws around the screen centre.
//!
//! Tier B (which item, which count, where); the models are drawn by `cw_render::passes`.
//!
//! # Map
//!
//! | Original | Here |
//! |---|---|
//! | `update` 0x00491f65..0x0049208e / 0x0049221c..0x0049228f: the quick-item button's `count` | [`button_count_text`], [`apply_button_count`] |
//! | `render` 0x004bac82..0x004bb043 (in the HUD branch, after the minimap): the Tab wheel | [`QuickWheel`] |

use glam::Vec2;

use super::gui_models::{item_model, GuiModelInputs};
use super::item_preview::{quick_items, wrap_index};
use super::present_hud::set_named_text;
use super::{GameUi, GameView};
use cw_render::passes::GuiModel;

/// `update` 0x00491f16..0x0049228f: the text of the quick-item button's `count` child. With
/// quick items (0x0047ae10) and the cursor captured (`isCursorFree` false, `[esp+0x17]`), the
/// selected stack's count (`wostringstream << count`, 0x00491fd1..0x00492062); otherwise one
/// space (0x006fd844, 0x0049221c).
pub fn button_count_text(game: &GameView, cursor_free: bool) -> String {
    let list = quick_items(game);
    if list.is_empty() || cursor_free {
        return " ".to_string();
    }
    let (t, i) = list[wrap_index(game.quick_index, list.len())];
    game.inventory[t][i].count.to_string()
}

/// `Node::setChildText("count", text, 1)` 0x00636a00 on the last quick-bar slot (`GC+0x8007fc`
/// `.back()`, 0x0046f430: the `quickitembutton` pushed after the six ability buttons).
pub fn apply_button_count(ui: &mut GameUi, game: &GameView) {
    let Some(&n) = ui.m.quick_slots.last() else { return };
    let free = super::flow::is_cursor_free(ui, game);
    let text = button_count_text(game, free);
    set_named_text(&mut ui.gui, n, "count", &text);
}

/// `GC+0x800a48` (radius) and `GC+0x800a4c` (angle, degrees) of the Tab wheel.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct QuickWheel {
    pub radius: f32,
    pub angle: f32,
}

impl QuickWheel {
    /// `render` 0x004bac91..0x004bb039 for `n` quick items. With the Tab menu open
    /// (`GC+0x800a40`): the radius eases to 200 and the angle to `index * -360 / n` by
    /// `0x004aba20(.., dt, 0.01)` (`dt` = `GC+0x8006e8`); item `i` sits at
    /// `a = i * 2π / n + angle * π / 180 - π / 2` (double, then float),
    /// `(cos a * r + W * 0.5, sin a * r + H * 0.5)`. Closed, the radius goes to 0 and nothing
    /// is drawn (the angle stays).
    pub fn frame(&mut self, open: bool, index: i32, n: usize, dt: i32, screen: [i32; 2]) -> Vec<Vec2> {
        let mut out = Vec::new();
        if !open {
            // 0x004bb039.
            self.radius = 0.0;
            return out;
        }
        crate::player::lerp_scalar(&mut self.radius, 200.0, dt, 0.01);
        // 0x004bace2: `(float)index * -360.0 / (float)n`.
        let target = index as f32 * -360.0f32 / n as f32;
        crate::player::lerp_scalar(&mut self.angle, target, dt, 0.01);
        let (w, h) = (screen[0] as f32, screen[1] as f32);
        for i in 0..n {
            let a = (f64::from(i as i32) * std::f64::consts::TAU / n as f64 + f64::from(self.angle) * std::f64::consts::PI / 180.0 - std::f64::consts::FRAC_PI_2) as f32;
            // 0x00424b50 / 0x0040e420: `sin` / `cos` of the float widened to double.
            let s = cw_math::sin(f64::from(a)) as f32;
            let y = s * self.radius + h * 0.5f32;
            let c = cw_math::cos(f64::from(a)) as f32;
            let x = c * self.radius + w * 0.5f32;
            out.push(Vec2::new(x, y));
        }
        out
    }

    /// The wheel's `0x004758c0(x, y, GC+0x800a1c, 0.09, item, 0)` calls (0x004bb01d), white
    /// (0x00448280 before the loop), in quick-item order.
    pub fn models(&mut self, ui: &GameUi, game: &GameView, dt: i32, screen: [i32; 2], inp: &GuiModelInputs) -> Vec<GuiModel> {
        let list = quick_items(game);
        let pos = self.frame(ui.flag_800a40, game.quick_index, list.len(), dt, screen);
        let mut out = Vec::new();
        for (p, &(t, i)) in pos.iter().zip(&list) {
            out.extend(item_model(0x004b_b01d, *p, 0.09, 0.0, &game.inventory[t][i].item, [1.0; 4], inp));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::ItemStack;

    fn potion(count: i32) -> ItemStack {
        let mut item = vec![0u8; 0x118];
        item[0] = 1;
        item[1] = 1;
        item[0x10] = 1;
        ItemStack { count, item }
    }

    fn game_with(stacks: Vec<ItemStack>) -> GameView {
        let mut g = GameView::default();
        g.player.0[0x180..0x184].copy_from_slice(&5i32.to_le_bytes());
        g.inventory = vec![stacks];
        g
    }

    #[test]
    fn count_is_the_selected_stack() {
        let g = game_with(vec![potion(2)]);
        assert_eq!(button_count_text(&g, false), "2");
        // A panel open: a space.
        assert_eq!(button_count_text(&g, true), " ");
        // Two stacks, the second selected.
        let mut g = game_with(vec![potion(2), ItemStack::empty(), potion(7)]);
        g.quick_index = 1;
        assert_eq!(button_count_text(&g, false), "7");
        g.quick_index = -1;
        assert_eq!(button_count_text(&g, false), "7");
        // No quick items.
        let g = game_with(vec![ItemStack::empty()]);
        assert_eq!(button_count_text(&g, false), " ");
    }

    #[test]
    fn button_count_reaches_the_count_node() {
        use cw_ui::widget::NodeSource;
        let mut ui = GameUi::default();
        let b = ui.gui.add_plain_node(None, "quickitembutton");
        let c = ui.gui.add_node(Some(b), NodeSource { name: "count".into(), visible: true, text: Some("1".into()), ..Default::default() });
        ui.m.quick_slots = vec![b];
        let g = game_with(vec![potion(2)]);
        apply_button_count(&mut ui, &g);
        assert_eq!(ui.gui.nodes[c].text, Some("2".encode_utf16().collect()));
    }

    #[test]
    fn wheel_eases_open_and_places_items_round_the_centre() {
        let mut w = QuickWheel::default();
        assert!(w.frame(false, 0, 3, 16, [800, 600]).is_empty());
        // A long open frame: radius near 200, the first item straight up from the centre.
        let p = w.frame(true, 0, 3, 5000, [800, 600]);
        assert_eq!(p.len(), 3);
        assert!((w.radius - 200.0).abs() < 0.01);
        assert!((p[0].x - 400.0).abs() < 0.01 && (p[0].y - 100.0).abs() < 0.1, "{:?}", p[0]);
        // Selecting the second item turns the wheel by -120 degrees: it comes to the top.
        let p = w.frame(true, 1, 3, 5000, [800, 600]);
        assert!((w.angle + 120.0).abs() < 0.01);
        assert!((p[1].x - 400.0).abs() < 0.1 && (p[1].y - 100.0).abs() < 0.1, "{:?}", p[1]);
        // Closing drops the radius.
        w.frame(false, 1, 3, 16, [800, 600]);
        assert_eq!(w.radius, 0.0);
    }
}
