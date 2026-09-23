//! The `InventoryWidget` callbacks: the `MemberFunctionConnection<cube::InventoryWidget>`
//! objects that `Widget::connect` 0x004c1a90 hangs on the tab buttons (`LEFT_PRESS` →
//! `switchTab` 0x004c5a60, connected by `setTabs` 0x004c6140), the up / down buttons
//! (`LEFT_PRESS` → `scrollUp` 0x004c60f0 / `scrollDown` 0x004c5a00) and the scroll button
//! (`MOUSE_MOVE` → `dragScroll` 0x004c5bb0), the last three through `connectByName`
//! 0x004c1a10 in the ctor 0x004c1bb0, for the bag (`GC+0x800954`), crafting (`+0x800958`)
//! and shop (`+0x80095c`) widgets.
//!
//! 0x004c1a90 is recursive (its fifth argument, 1 at every call site here): the connection is
//! also stored on every widget below the button's node (0x00629140 collects them), each with
//! the InventoryWidget as target; so a signal from any widget in the button's subtree runs the
//! callback. The engine fires the signal on the node's owner widget (0x00653620), which is how
//! it reaches [`GameUi::apply`]; the callbacks run synchronously inside the engine's inject
//! call in the original, so `MOUSE_MOVE` is handled right after `Engine::injectMouseMove`
//! ([`GameUi::on_mouse_moved`]) where the engine's previous cursor is still the one it used.

use cw_ui::widget::{event, NodeId, UiEvent, WidgetId};
use glam::Vec2;

use super::inventory_widget::{InventoryWidget, SelectorPlacement};
use super::members::{find_node, InventoryParts};
use super::{GameUi, GameView, ItemStack, UiAction};

/// Which InventoryWidget: 0 bag, 2 crafting, 3 shop (the widget's `+0x194` type).
const KINDS: [i32; 3] = [0, 2, 3];

impl GameUi {
    fn inv_parts(&self, kind: i32) -> &InventoryParts {
        match kind {
            0 => &self.m.bag_parts,
            2 => &self.m.crafting_parts,
            _ => &self.m.shop_parts,
        }
    }

    fn inv_widget_id(&self, kind: i32) -> Option<WidgetId> {
        match kind {
            0 => self.m.inventory,
            2 => self.m.crafting,
            _ => self.m.shop,
        }
    }

    /// The widget's data (`+0x160`): the bag `creature+0x11dc`, crafting `GC+0x800adc`, the
    /// shop `GC+0x800c0c` (never null for these three).
    fn inv_pages(&self, kind: i32, game: &GameView) -> Vec<Vec<ItemStack>> {
        match kind {
            0 => game.inventory.clone(),
            2 => self.crafting.tabs.clone(),
            _ => self.inv.shop_data.pages.clone(),
        }
    }

    fn inv_widget_mut(&mut self, kind: i32) -> &mut InventoryWidget {
        match kind {
            0 => &mut self.inv.bag,
            2 => &mut self.inv.crafting,
            _ => &mut self.inv.shop,
        }
    }

    /// The node a part's `connectByName` 0x004c1a10 connected: the node named `name` inside
    /// the clone (the clone itself when it carries the name).
    fn inv_named(&self, n: Option<NodeId>, name: &str) -> Option<NodeId> {
        n.map(|n| find_node(&self.gui, n, name).unwrap_or(n))
    }

    /// Whether `widget` is one of the widgets the recursive connect put the callback on: the
    /// widget of `button` or of a node below it.
    fn inv_in_subtree(&self, button: Option<NodeId>, widget: WidgetId) -> bool {
        let wn = self.gui.widgets[widget].node;
        button.is_some_and(|b| self.gui.is_ancestor_or_self(b, Some(wn)))
    }

    /// `+0x178`'s widget: the scroll button (thumb) that 0x004c5bb0 / 0x004c64c0 move.
    fn inv_thumb_button(&self, kind: i32) -> Option<WidgetId> {
        let parts = self.inv_parts(kind);
        let n = parts.scroll.and_then(|n| find_node(&self.gui, n, "scrollbutton"));
        n.and_then(|n| self.gui.nodes[n].widget)
    }

    /// `0x004c6610` tail: the selector's Display visibility, and `setPosition` when shown.
    fn inv_place_selector(&mut self, kind: i32, sel: Option<SelectorPlacement>) {
        let (Some(p), Some(n)) = (sel, self.inv_parts(kind).selector) else { return };
        self.gui.nodes[n].visible = p.visible;
        if p.visible && let Some(sw) = self.gui.nodes[n].widget {
            self.gui.set_position(sw, p.pos, true);
        }
    }

    /// `0x004c64c0`: `setRect(x, y, w, h)` (0x0062bb20) on the scroll button from the current
    /// tab's scroll position; remembered in [`InventoryWidget::thumb_written`] so that
    /// `present_panels::apply` only rewrites it when it changes.
    pub(super) fn inv_place_thumb(&mut self, kind: i32) {
        let Some(b) = self.inv_thumb_button(kind) else { return };
        let size = self.widget_size(self.inv_widget_id(kind));
        let bw = self.gui.local_size(b).x;
        let Some(r) = self.inv_widget_mut(kind).scroll_thumb(size, bw) else { return };
        self.gui.set_position_xy(b, r[0], r[1], true);
        let want = Vec2::new(r[2], r[3]);
        if self.gui.local_size(b) != want {
            self.gui.set_size(b, want, true);
        }
        self.inv_widget_mut(kind).thumb_written = Some(r);
    }

    /// A `LEFT_PRESS` signal on a tab, up or down button of one of the three
    /// InventoryWidgets; `false` when `widget` is none of them.
    pub(super) fn inventory_left_press(&mut self, widget: WidgetId, game: &GameView, out: &mut Vec<UiAction>) -> bool {
        for kind in KINDS {
            let parts = self.inv_parts(kind);
            let tab = parts.tabs.iter().any(|&t| self.inv_in_subtree(Some(t), widget));
            let up = self.inv_in_subtree(self.inv_named(parts.up, "upbutton"), widget);
            let down = self.inv_in_subtree(self.inv_named(parts.down, "downbutton"), widget);
            if !(tab || up || down) {
                continue;
            }
            let pages = self.inv_pages(kind, game);
            let size = self.widget_size(self.inv_widget_id(kind));
            let wdg = self.inv_widget_mut(kind);
            if tab {
                // 0x004c5a60: select(0), the thumb (0x004c64c0), sound 0x56 (0x00484320),
                // then 0x004815c0 (the shop rebuilds its pages, 0x004a2300).
                let (actions, sel) = wdg.switch_tab(&pages, size, kind == 3);
                if !actions.is_empty() {
                    self.inv_place_selector(kind, sel);
                    self.inv_place_thumb(kind);
                    out.extend(actions);
                }
                return true;
            }
            // 0x004c60f0 / 0x004c5a00: select(0) then the thumb, only when the row moved.
            let before = wdg.scroll.clone();
            let sel = if up { wdg.scroll_up(&pages, size) } else { wdg.scroll_down(&pages, size) };
            if self.inv_widget_mut(kind).scroll != before {
                self.inv_place_selector(kind, sel);
                self.inv_place_thumb(kind);
            }
            return true;
        }
        false
    }

    /// Runs the `MOUSE_MOVE` signals the last `Gui::inject_mouse_move` queued on the three
    /// scroll buttons (`dragScroll` 0x004c5bb0) and removes them from the queue. Call right
    /// after `inject_mouse_move`: the drag reads the engine's cursor motion (`+0xd8 - +0xe0`)
    /// and left button (`+0xf4` bit 0) as they were when the engine fired the signal.
    pub fn on_mouse_moved(&mut self) {
        let events = std::mem::take(&mut self.gui.events);
        let mut kept = Vec::with_capacity(events.len());
        for e in events {
            if let UiEvent::Signal { widget, event: ev } = e
                && ev == event::MOUSE_MOVE
                && let Some(kind) = KINDS.iter().copied().find(|&k| {
                    let n = self.inv_named(self.inv_parts(k).scroll, "scrollbutton");
                    self.inv_in_subtree(n, widget)
                })
            {
                self.inventory_drag(kind);
                continue;
            }
            kept.push(e);
        }
        // Anything queued meanwhile goes after (nothing queues here today).
        kept.append(&mut self.gui.events);
        self.gui.events = kept;
    }

    /// 0x004c5bb0: the thumb follows the cursor (not snapped to a row) and the row follows
    /// the thumb.
    fn inventory_drag(&mut self, kind: i32) {
        let Some(b) = self.inv_thumb_button(kind) else { return };
        let size = self.widget_size(self.inv_widget_id(kind));
        let dy = self.gui.cursor.y - self.gui.prev_cursor.y;
        let pos = self.gui.get_position(b);
        let left = self.gui.left_down;
        let Some((p, sel)) = self.inv_widget_mut(kind).drag_scroll(left, dy, pos, size) else { return };
        self.inv_place_selector(kind, sel);
        // `0x0062a650(x, y, 1)` on +0x178's widget.
        self.gui.set_position_xy(b, p.x, p.y, true);
        // The original does not run 0x004c64c0 here: the thumb stays where the cursor put it
        // until the next event that places it. Mark the current rectangle as written so the
        // per-frame present does not snap it back.
        let bw = self.gui.local_size(b).x;
        let w = self.inv_widget_mut(kind);
        w.thumb_written = w.scroll_thumb(size, bw);
    }
}

#[cfg(test)]
mod tests {
    use super::super::members::NoPlx;
    use super::super::*;
    use cw_ui::widget::{Gui, UiEvent, WidgetSource, WidgetSourceKind};

    fn st(ty: u8) -> ItemStack {
        let mut s = ItemStack::empty();
        s.count = 1;
        s.item[0] = ty;
        s.item[0x10] = 1;
        s
    }

    fn button(ui: &mut GameUi, parent: Option<cw_ui::widget::NodeId>, name: &str) -> (cw_ui::widget::NodeId, cw_ui::widget::WidgetId) {
        let n = ui.gui.add_plain_node(parent, name);
        let w = ui.gui.add_widget(n, &WidgetSource { kind: WidgetSourceKind::Button { button_type: 0 }, ..Default::default() });
        (n, w)
    }

    /// A GameUi without `.plx` files, with a bag InventoryWidget node of 400x285 and its
    /// parts (two tabs, up / down / scroll buttons) built by hand.
    fn synthetic() -> (GameUi, GameView) {
        let mut gui = Gui::new();
        gui.viewport = glam::IVec2::new(1280, 720);
        let mut game = GameView::default();
        let mut ui = GameUi::new(gui, &mut NoPlx, &game);
        let root = ui.gui.root;
        let (inn, inw) = button(&mut ui, root, "inventory");
        ui.gui.set_size(inw, Vec2::new(400.0, 285.0), true);
        ui.m.inventory = Some(inw);
        for kind in [0, 3] {
            let (wn, _) = if kind == 0 { (inn, inw) } else {
                let (n, w) = button(&mut ui, root, "shop");
                ui.gui.set_size(w, Vec2::new(400.0, 285.0), true);
                ui.m.shop = Some(w);
                (n, w)
            };
            let mut parts = members::InventoryParts::default();
            for _ in 0..2 {
                let (t, _) = button(&mut ui, Some(wn), "tab");
                parts.tabs.push(t);
            }
            parts.up = Some(button(&mut ui, Some(wn), "upbutton").0);
            parts.down = Some(button(&mut ui, Some(wn), "downbutton").0);
            parts.scroll = Some(button(&mut ui, Some(wn), "scrollbutton").0);
            if kind == 0 { ui.m.bag_parts = parts } else { ui.m.shop_parts = parts }
        }
        // 200 items in tab 0: 8 columns, 5 visible rows, 21 scroll positions.
        game.inventory = vec![vec![st(1); 200], vec![st(2); 3]];
        (ui, game)
    }

    fn press(ui: &mut GameUi, game: &mut GameView, n: cw_ui::widget::NodeId) -> Vec<UiAction> {
        let w = ui.gui.nodes[n].widget.unwrap();
        ui.apply(&[UiEvent::Signal { widget: w, event: event::LEFT_PRESS }], game)
    }

    #[test]
    fn tab_press_switches_to_hovered_tab() {
        let (mut ui, mut game) = synthetic();
        // 0x004c2050 recorded tab button 1 as the last hovered.
        ui.inv.bag.hovered_tab_button = 1;
        let t = ui.m.bag_parts.tabs[1];
        let a = press(&mut ui, &mut game, t);
        assert_eq!(ui.inv.bag.tab, 1);
        assert_eq!((ui.inv.bag.selected_tab, ui.inv.bag.selected_index), (1, 0));
        assert_eq!(a, vec![UiAction::PlaySound { id: 0x56, volume: 1.0, pitch: 1.0 }]);
        // Same tab again: nothing.
        assert!(press(&mut ui, &mut game, t).is_empty());
    }

    #[test]
    fn shop_tab_press_refreshes_shop() {
        let (mut ui, mut game) = synthetic();
        ui.inv.shop.hovered_tab_button = 1;
        let t = ui.m.shop_parts.tabs[1];
        let a = press(&mut ui, &mut game, t);
        assert_eq!(ui.inv.shop.tab, 1);
        assert!(a.contains(&UiAction::RefreshShop));
    }

    #[test]
    fn child_widget_of_tab_also_switches() {
        // 0x004c1a90 connects recursively: a widget under the tab node runs the callback too.
        let (mut ui, mut game) = synthetic();
        let t = ui.m.bag_parts.tabs[1];
        let (c, _) = button(&mut ui, Some(t), "frame");
        ui.inv.bag.hovered_tab_button = 1;
        press(&mut ui, &mut game, c);
        assert_eq!(ui.inv.bag.tab, 1);
    }

    #[test]
    fn up_down_scroll_and_place_thumb() {
        let (mut ui, mut game) = synthetic();
        let (up, down) = (ui.m.bag_parts.up.unwrap(), ui.m.bag_parts.down.unwrap());
        assert!(press(&mut ui, &mut game, down).is_empty());
        assert_eq!(ui.inv.bag.scroll[0], 1);
        assert_eq!(ui.inv.bag.rows, 21);
        let b = ui.gui.nodes[ui.m.bag_parts.scroll.unwrap()].widget.unwrap();
        // 0x004c64c0: y = (215 - 10) * 1 / 20 + 35 = 45, height 215 / 21 = 10.
        assert_eq!(ui.gui.get_position(b), Vec2::new(372.0, 45.0));
        assert_eq!(ui.gui.local_size(b).y, 10.0);
        press(&mut ui, &mut game, up);
        press(&mut ui, &mut game, up);
        assert_eq!(ui.inv.bag.scroll[0], 0);
    }

    #[test]
    fn drag_moves_thumb_and_row_without_snap() {
        let (mut ui, game) = synthetic();
        ui.inv.bag.update_rows(&game.inventory, Vec2::new(400.0, 285.0));
        ui.inv_place_thumb(0);
        let b = ui.gui.nodes[ui.m.bag_parts.scroll.unwrap()].widget.unwrap();
        assert_eq!(ui.gui.get_position(b).y, 35.0);
        ui.gui.left_down = true;
        ui.gui.prev_cursor = Vec2::new(380.0, 40.0);
        ui.gui.cursor = Vec2::new(380.0, 140.0);
        ui.gui.events.push(UiEvent::Signal { widget: b, event: event::MOUSE_MOVE });
        ui.on_mouse_moved();
        assert!(ui.gui.events.is_empty());
        // y = 35 + 100 = 135, row = (100 * 20) / 205 = 9.
        assert_eq!(ui.inv.bag.scroll[0], 9);
        assert_eq!(ui.gui.get_position(b), Vec2::new(372.0, 135.0));
        // The per-frame present keeps the dragged position (0x004c64c0 is not rerun).
        let out = FrameOutput { bag: Some(inventory_widget::InventoryFrame { visible: true, ..Default::default() }), ..Default::default() };
        present_panels::apply(&mut ui, &game, &out);
        assert_eq!(ui.gui.get_position(b).y, 135.0);
        // Without the left button nothing moves.
        ui.gui.left_down = false;
        ui.gui.events.push(UiEvent::Signal { widget: b, event: event::MOUSE_MOVE });
        ui.on_mouse_moved();
        assert_eq!(ui.inv.bag.scroll[0], 9);
    }

    /// With the game's `.plx` files: the pointer over a bag tab, a left press reaches the tab
    /// callback through the engine's hit test and signal. Skipped without `CW_GAME_DIR`.
    #[test]
    fn real_tab_click_switches_bag_tab() {
        let Some(dir) = std::env::var_os("CW_GAME_DIR").map(std::path::PathBuf::from) else {
            eprintln!("CW_GAME_DIR not set; skipped");
            return;
        };
        let mut plx = crate::ui::plx_files::GamePlxLoader::new(&dir);
        let mut gui = Gui::new();
        gui.viewport = glam::IVec2::new(1280, 720);
        let mut game = GameView::default();
        game.inventory = vec![vec![st(1); 3]; 4];
        let mut ui = GameUi::new(gui, &mut plx, &game);
        ui.set_visible(ui.m.inventory_panel, true);
        let t = ui.m.bag_parts.tabs[2];
        let w = ui.gui.nodes[t].widget.unwrap();
        let p = ui.gui.widget_world(w).transform_point2(Vec2::new(ui.gui.width(w), ui.gui.height(w)) * 0.5);
        ui.gui.inject_mouse_move(p.x, p.y, None);
        assert!(ui.is_hovered(w));
        // 0x004c2050's tab loop records the hovered tab button.
        ui.inv.bag.hovered_tab_button = 2;
        ui.gui.events.clear();
        ui.gui.inject_left_down(None);
        let events = std::mem::take(&mut ui.gui.events);
        let a = ui.apply(&events, &mut game);
        assert_eq!(ui.inv.bag.tab, 2);
        assert!(a.contains(&UiAction::PlaySound { id: 0x56, volume: 1.0, pitch: 1.0 }));
    }
}
