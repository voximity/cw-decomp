//! `cube::InventoryWidget` (vtable 0x00702bb4, ctor `Cube.exe 0x004c1bb0`, 0x1b8 bytes): the
//! item grid of the bag (`GC+0x800954`, type 0), the crafting panel (`GC+0x800958`, type 2)
//! and the shop (`GC+0x80095c`, type 3); its per-frame body (slot 1, 0x004c2050), the layout
//! (slot 10, 0x004c5d50, geometry in cw-ui `inventory_layout`), the tab/scroll callbacks and
//! the shop's data (`GC+0x800c0c`, the buy-back list `GC+0x800d3c`, rebuilt by 0x004a2300).
//!
//! | Field | Offset |
//! |---|---|
//! | data: a `cube::Inventory*` (tabs at +0, coins +0x128, platinum +0x12c) | +0x160 |
//! | GameController* | +0x164 |
//! | cell template (`itembox`), selector (`itemselector`), up, down, scroll button nodes | +0x168, +0x16c, +0x170, +0x174, +0x178 |
//! | the tab button nodes (`std::list`, set by 0x004c6140) | +0x17c |
//! | hovered cell (tab, index) | +0x184, +0x188 |
//! | selected cell (tab, index) | +0x18c, +0x190 |
//! | type (0 bag, 2 crafting, 3 shop) | +0x194 |
//! | per-tab first visible row (`std::vector<int>`) | +0x198 |
//! | scroll positions of the current tab | +0x1a4 |
//! | cell size (ints, default 40; the template's size) | +0x1a8, +0x1ac |
//! | last hovered tab button | +0x1b0 |
//! | current tab | +0x1b4 |
//!
//! Callbacks: tab buttons `LEFT_PRESS` → [`InventoryWidget::switch_tab`] (0x004c5a60), up
//! button → [`InventoryWidget::scroll_up`] (0x004c60f0), down button →
//! [`InventoryWidget::scroll_down`] (0x004c5a00), scroll button `MOUSE_MOVE` →
//! [`InventoryWidget::drag_scroll`] (0x004c5bb0). Slot 5 (0x004c59a0) is the plain
//! `0 <= p < size` hit test.

use glam::{IVec2, Vec2};

use super::inventory::{cvtt, item_level, price};
use super::{ItemStack, UiAction};

/// The cube::InventoryWidget fields (see the module table).
#[derive(Clone, Debug, PartialEq)]
pub struct InventoryWidget {
    /// +0x194: 0 bag, 1 (unused: captions "Equipment"), 2 crafting, 3 shop.
    pub kind: i32,
    /// +0x16c present (the crafting widget's `itemselector`).
    pub has_selector: bool,
    /// +0x170 / +0x174 / +0x178 present (up, down, scroll buttons).
    pub has_up: bool,
    /// See [`InventoryWidget::has_up`].
    pub has_down: bool,
    /// See [`InventoryWidget::has_up`].
    pub has_scroll: bool,
    /// +0x184: hovered tab, -1 none (written every frame by [`InventoryWidget::frame`]).
    pub hovered_tab: i32,
    /// +0x188: hovered index, -1 none.
    pub hovered_index: i32,
    /// +0x18c: selected tab (crafting selection).
    pub selected_tab: i32,
    /// +0x190: selected index.
    pub selected_index: i32,
    /// +0x198: per tab, the first visible row.
    pub scroll: Vec<i32>,
    /// +0x1a4: the number of scroll positions of the current tab (1 when it fits).
    pub rows: i32,
    /// +0x1a8 / +0x1ac: cell size.
    pub cell: IVec2,
    /// +0x1b0: the tab button last hovered (-1 before any).
    pub hovered_tab_button: i32,
    /// +0x1b4: the current tab.
    pub tab: i32,
    /// Port-only: the scroll thumb rectangle 0x004c64c0 last wrote (`None` before any). The
    /// original places the thumb only on the events that change it (the callbacks, the
    /// layout 0x004c5d50, the item and shop updates) and `dragScroll` 0x004c5bb0 leaves it
    /// off the row grid; `present_panels::apply` recomputes it each frame and writes it only
    /// when it differs from this.
    pub thumb_written: Option<[f32; 4]>,
}

impl InventoryWidget {
    /// The ctor 0x004c1bb0 for type `kind`: hover (-1, -1), selection (0, 0), one scroll
    /// position, cell 40x40 or, with a cell template, `(int)` of its width and height
    /// (`__ftol2` 0x0068d910), tab 0.
    pub fn new(kind: i32, template_size: Option<Vec2>) -> Self {
        let cell = template_size.map_or(IVec2::new(0x28, 0x28), |s| IVec2::new(cvtt(s.x), cvtt(s.y)));
        InventoryWidget {
            kind,
            has_selector: false,
            has_up: false,
            has_down: false,
            has_scroll: false,
            hovered_tab: -1,
            hovered_index: -1,
            selected_tab: 0,
            selected_index: 0,
            scroll: Vec::new(),
            rows: 1,
            cell,
            hovered_tab_button: -1,
            tab: 0,
            thumb_written: None,
        }
    }

    /// `(cols, visible rows)` for widget size `size` (0x0062f600 / 0x006291d0):
    /// `(int)((w - 10) / (cellW + 5))`, `(int)((h - 40) / (cellH + 5))`.
    pub fn grid(&self, size: Vec2) -> (i32, i32) {
        (cvtt((size.x - 10.0f32) / (self.cell.x + 5) as f32), cvtt((size.y - 40.0f32) / (self.cell.y + 5) as f32))
    }

    fn scroll_of_tab(&self) -> i32 {
        if self.tab >= 0 { self.scroll.get(self.tab as usize).copied().unwrap_or(0) } else { 0 }
    }

    /// 0x004c6350: resizes the per-tab rows to the tab count (new ones 0) and recomputes the
    /// scroll positions of the current tab: 1 without tabs or past the last; 0 (and row 0) for
    /// an empty tab; else `(len - 1) / cols - visible rows + 2` (unsigned division), at least
    /// 1, the tab's row clamped below it.
    pub fn update_rows(&mut self, pages: &[Vec<ItemStack>], size: Vec2) {
        let (cols, vis) = self.grid(size);
        self.scroll.resize(pages.len(), 0);
        if pages.is_empty() || pages.len() as i32 <= self.tab {
            self.rows = 1;
            return;
        }
        if self.tab < 0 {
            // (The original indexes out of the vector; not reachable through its callers.)
            self.rows = 1;
            return;
        }
        let t = self.tab as usize;
        let len = pages[t].len() as u32;
        if len == 0 {
            self.rows = 0;
            self.scroll[t] = 0;
            return;
        }
        // (A zero column count divides by zero in the original.)
        let q = if cols == 0 { 0 } else { (len - 1) / cols as u32 };
        let mut r = (q.wrapping_sub(vis as u32) as i32).wrapping_add(2);
        if r < 1 {
            r = 1;
        }
        self.rows = r;
        if r - 1 < self.scroll[t] {
            self.scroll[t] = r - 1;
        }
    }

    /// 0x004c6610(select): with `select`, the hovered cell becomes the selection (nothing
    /// without a hovered index) with sound 0x55; then the selector (when present) is shown at
    /// the selected cell when it lies in the visible rows of the current tab, else hidden.
    /// Returns the sound and the selector placement (`None`: no selector or nothing done).
    pub fn select(&mut self, select: bool, size: Vec2) -> (Vec<UiAction>, Option<SelectorPlacement>) {
        let mut out = Vec::new();
        if select {
            if self.hovered_index < 0 {
                return (out, None);
            }
            self.selected_tab = self.hovered_tab;
            self.selected_index = self.hovered_index;
            out.push(UiAction::PlaySound { id: 0x55, volume: 1.0, pitch: 1.0 });
        }
        if !self.has_selector {
            return (out, None);
        }
        let (cols, vis) = self.grid(size);
        let first = self.scroll_of_tab().wrapping_mul(cols);
        let s = self.selected_index;
        if -1 < s && first <= s && s < vis.wrapping_mul(cols).wrapping_add(first) && cols != 0 {
            let k = s - first;
            let pos = Vec2::new(((self.cell.x + 5) * (k % cols) + 10) as f32, ((self.cell.y + 5) * (k / cols) + 0x28) as f32);
            (out, Some(SelectorPlacement { visible: true, pos }))
        } else {
            (out, Some(SelectorPlacement { visible: false, pos: Vec2::ZERO }))
        }
    }

    /// 0x004c60f0 (up button): one row up when the current tab has rows and is not at the
    /// top; then the selector and the scroll thumb follow.
    pub fn scroll_up(&mut self, pages: &[Vec<ItemStack>], size: Vec2) -> Option<SelectorPlacement> {
        self.update_rows(pages, size);
        let t = self.tab;
        if t >= 0 && (t as usize) < self.scroll.len() && self.scroll[t as usize] > 0 {
            self.scroll[t as usize] -= 1;
            return self.select(false, size).1;
        }
        None
    }

    /// 0x004c5a00 (down button): one row down while below `rows - 1`.
    pub fn scroll_down(&mut self, pages: &[Vec<ItemStack>], size: Vec2) -> Option<SelectorPlacement> {
        self.update_rows(pages, size);
        let t = self.tab;
        if t >= 0 && (t as usize) < self.scroll.len() && self.scroll[t as usize] < self.rows - 1 {
            self.scroll[t as usize] += 1;
            return self.select(false, size).1;
        }
        None
    }

    /// 0x004c5bb0 (scroll button `MOUSE_MOVE`): with the left button held and `rows > 0`, the
    /// thumb follows the cursor's vertical motion (`cursor_dy` = engine `+0xd8 - +0xe0`) from
    /// its position `button_pos` (0x0062b510), clamped to `[35, (h - 70) - (h - 70) / rows +
    /// 35]` (`h - 70` truncated); the row is `((y - 35) * (rows - 1)) / range` clamped to
    /// `[0, rows - 1]`. Returns the thumb's new position (`setPosition(x, y)`) and the
    /// selector placement. (The original also prints `h - 70` and the thumb height to
    /// stdout.)
    pub fn drag_scroll(&mut self, left_down: bool, cursor_dy: f32, button_pos: Vec2, size: Vec2) -> Option<(Vec2, Option<SelectorPlacement>)> {
        if !left_down || self.rows <= 0 {
            return None;
        }
        let hh = cvtt(size.y - 70.0f32);
        let thumb = hh / self.rows;
        let mut y = cvtt(cursor_dy + button_pos.y);
        if y < 0x23 {
            y = 0x23;
        }
        let range = hh - thumb;
        if range + 0x23 < y {
            y = range + 0x23;
        }
        let t = self.tab.max(0) as usize;
        if t < self.scroll.len() {
            if range > 0 {
                self.scroll[t] = ((y - 0x23) * (self.rows - 1)) / range;
            }
            if self.scroll[t] < 0 {
                self.scroll[t] = 0;
            }
            if self.rows <= self.scroll[t] {
                self.scroll[t] = self.rows - 1;
            }
        }
        let sel = self.select(false, size).1;
        Some((Vec2::new(button_pos.x, y as f32), sel))
    }

    /// 0x004c64c0: the scroll thumb's rectangle (`0x0062bb20(x, y, w, h)` on the scroll
    /// button): x `width - 28`, height `(int)(h - 70) / rows`, y 35 for one position else
    /// `((int)(h - 70) - height) * row / (rows - 1) + 35`; `thumb_width` is the button's own
    /// width. `None` without a scroll button, rows or tabs.
    pub fn scroll_thumb(&self, size: Vec2, thumb_width: f32) -> Option<[f32; 4]> {
        if !self.has_scroll || self.rows == 0 || self.scroll.is_empty() {
            return None;
        }
        let hh = cvtt(size.y - 70.0f32);
        let thumb = hh / self.rows;
        let y = if self.rows > 1 { ((hh - thumb) * self.scroll_of_tab()) / (self.rows - 1) + 0x23 } else { 0x23 };
        Some([size.x - 28.0f32, y as f32, thumb_width, thumb as f32])
    }

    /// 0x004c5a60 (a tab button's `LEFT_PRESS`): switches to the last hovered tab button
    /// ([`InventoryWidget::hovered_tab_button`]) when it differs: the selection becomes the
    /// first visible cell of the new tab (or (-1, -1) for an empty or missing tab), sound
    /// 0x56 at the listener, and `0x004815c0`: the shop widget rebuilds its pages
    /// ([`UiAction::RefreshShop`], pass `is_shop`). (The original prints "tab: old new".)
    pub fn switch_tab(&mut self, pages: &[Vec<ItemStack>], size: Vec2, is_shop: bool) -> (Vec<UiAction>, Option<SelectorPlacement>) {
        let mut out = Vec::new();
        let h = self.hovered_tab_button;
        if h < 0 || self.tab == h {
            return (out, None);
        }
        self.tab = h;
        self.update_rows(pages, size);
        let t = self.tab;
        let empty = pages.get(t as usize).is_none_or(|p| p.is_empty());
        if empty || t < 0 || self.scroll.len() as i32 <= t {
            self.selected_tab = -1;
            self.selected_index = -1;
        } else {
            let (cols, _) = self.grid(size);
            self.selected_tab = t;
            self.selected_index = self.scroll[t as usize].wrapping_mul(cols);
        }
        let sel = self.select(false, size).1;
        out.push(UiAction::PlaySound { id: 0x56, volume: 1.0, pitch: 1.0 });
        if is_shop {
            out.push(UiAction::RefreshShop);
        }
        (out, sel)
    }

    /// `InventoryWidget` slot 1, 0x004c2050: one frame. Nothing without data. Otherwise the
    /// scroll positions are recomputed (0x004c6350), the hover reset, and when the current tab
    /// exists: the item models of the visible cells (the hovered one enlarged; hover =
    /// `(tab, index)` of the cell under the engine cursor), the caption, the bag's money, the
    /// per-cell texts (stack counts, and for the crafting and shop widgets the name with its
    /// level and the shop price) and the up/down/scroll button colours. Then the tab buttons'
    /// colours (the hovered one also becomes [`InventoryWidget::hovered_tab_button`]) and the
    /// cursor's stack drawn at the cursor.
    pub fn frame(&mut self, input: &FrameInput) -> InventoryFrame {
        let mut f = InventoryFrame::default();
        let Some(pages) = input.pages else { return f };
        f.visible = true;
        self.update_rows(pages, input.size);
        self.hovered_tab = -1;
        self.hovered_index = -1;
        let ntabs = pages.len() as i32;
        if self.tab < ntabs && self.tab >= 0 {
            let t = self.tab as usize;
            let (cols, vis) = self.grid(input.size);
            let row0 = self.scroll_of_tab();
            let first = row0.wrapping_mul(cols);
            let mut last = first.wrapping_add(vis.wrapping_mul(cols));
            if (pages[t].len() as i32) < last {
                last = pages[t].len() as i32;
            }
            let (cw, ch) = (self.cell.x, self.cell.y);
            // 0x004c22fc: the models (render state 7 = 1 around them).
            for i in first.max(0)..last {
                let k = i - first;
                let x = cvtt(((cw + 5) * (k % cols)) as f32 + (input.origin.x + 10.0f32));
                let y = cvtt(((ch + 5) * (k / cols)) as f32 + (input.origin.y + 40.0f32));
                let mut scale = 0.03f32;
                let (cx, cy) = (input.cursor.x, input.cursor.y);
                if cx >= x as f32 && ((cw + x) as f32) > cx && cy >= y as f32 && ((ch + y) as f32) > cy {
                    self.hovered_tab = self.tab;
                    self.hovered_index = i;
                    scale = 0.045f32;
                }
                let s = &pages[t][i as usize];
                if s.count != 0 || self.kind == 2 || self.kind == 3 {
                    let color = if s.count < 0 { [0.0, 0.0, 0.0, 1.0] } else { [1.0; 4] };
                    if s.item[0] == 9 {
                        scale *= 0.75f32;
                    }
                    let half = ch / 2;
                    f.models.push(ItemModel { pos: Vec2::new((x + half) as f32, (y + half) as f32), scale, color: Some(color), item: s.item.clone() });
                }
            }
            // 0x004c25a9: the caption.
            let caption = match (self.kind, self.tab) {
                (0, 0) | (1, _) => Some("Equipment"),
                (0, 1) => Some("Items"),
                (0, 2) => Some("Ingredients"),
                (0, _) => None,
                (2, 0) => Some("Weapons"),
                (2, 1) => Some("Armor"),
                (2, 2) => Some("Amulets"),
                (2, 3) => Some("Cooking"),
                (2, 4) => Some("Alchemy"),
                (2, _) => Some("Formulas"),
                (3, _) => Some("Vendor"),
                _ => None,
            };
            // `<< caption << std::endl`: the text keeps the newline.
            let caption = caption.map_or(String::new(), |c| format!("{c}\n"));
            f.texts.extend(two_pass(&caption, Vec2::new(15.0, 25.0), 12.0, 3.0, 0, -1.0, WHITE));
            if self.kind == 0 {
                // 0x004c27f4: the bag's money (data+0x128 coins, +0x12c platinum).
                let c = input.coins;
                let (copper, silver, gold) = (c % 100, (c / 100) % 100, (c / 100) / 100);
                let x0 = cvtt(input.local_size.x - 30.0f32);
                let y0 = cvtt(input.local_size.y - 10.0f32);
                let p = |x: i32| Vec2::new(x as f32, y0 as f32);
                f.texts.extend(two_pass(&format!("{copper} C "), p(x0), 10.0, 2.0, 2, -1.0, COPPER));
                f.texts.extend(two_pass(&format!("{silver} S "), p(x0 - 0x28), 10.0, 2.0, 2, -1.0, SILVER));
                f.texts.extend(two_pass(&format!("{gold} G "), p(x0 - 0x50), 10.0, 2.0, 2, -1.0, GOLD));
                let y1 = cvtt(input.local_size.y - 10.0f32);
                f.texts.extend(two_pass(&format!("{}  Platinum Coins ", input.platinum), Vec2::new(15.0, y1 as f32), 10.0, 2.0, 0, -1.0, PLATINUM));
            }
            // 0x004c2c5a: the per-cell texts (widget-local positions).
            for i in first.max(0)..last {
                let k = i - first;
                let (row, col) = (k / cols, k % cols);
                let s = &pages[t][i as usize];
                let it = &s.item;
                let count_pos = Vec2::new(((cw + 5) * col + 6 + cw) as f32, ((ch + 5) * row + ch + 0x25) as f32);
                if it[0] != 0 && matches!(it[0], 1 | 0xa | 0xc | 0xd | 0xb | 0x15) && 0 < s.count {
                    f.texts.extend(two_pass(&format!("{}\n", s.count), count_pos, 10.0, 2.0, 2, -1.0, WHITE));
                }
                let name_pos = Vec2::new(((cw + 5) * col + 0x14 + ch) as f32, ((ch + 5) * row + 0x32) as f32);
                let wrap = ((cw - ch) - 0x14) as f32;
                if it[0] != 0 && self.kind == 2 {
                    if 0 < s.count {
                        f.texts.extend(two_pass(&format!("{}\n", s.count), count_pos, 10.0, 2.0, 2, -1.0, WHITE));
                    }
                    let name = name_text(it, input.item_name);
                    f.texts.extend(two_pass(&name, name_pos, 10.0, 2.0, 0x10, wrap, rarity_color(it)));
                }
                if it[0] != 0 && self.kind == 3 {
                    let name = name_text(it, input.item_name);
                    f.texts.extend(two_pass(&name, name_pos, 10.0, 2.0, 0x10, wrap, rarity_color(it)));
                    // 0x004c4f..: the price, right to left.
                    let v = price(it);
                    let (gold, silver, copper) = ((v / 100) / 100, (v / 100) % 100, v % 100);
                    let mut x = cw + 10 + (cw + 5) * col;
                    let y = ((ch + 5) * row + ch + 0x25) as f32;
                    if copper != 0 {
                        f.texts.extend(two_pass(&format!("{copper} C "), Vec2::new(x as f32, y), 10.0, 2.0, 2, -1.0, COPPER));
                        x -= 0x28;
                    }
                    if silver != 0 {
                        f.texts.extend(two_pass(&format!("{silver} S "), Vec2::new(x as f32, y), 10.0, 2.0, 2, -1.0, SILVER));
                        x -= 0x28;
                    }
                    if gold != 0 {
                        f.texts.extend(two_pass(&format!("{gold} G "), Vec2::new(x as f32, y), 10.0, 2.0, 2, -1.0, GOLD));
                    }
                }
            }
            // 0x004c5620: the buttons, cyan when hovered.
            let tint = |h: bool| [if h { 0.0 } else { 1.0 }, 1.0, 1.0, 1.0];
            f.up_color = self.has_up.then(|| tint(input.up_hovered));
            f.down_color = self.has_down.then(|| tint(input.down_hovered));
            f.scroll_color = self.has_scroll.then(|| tint(input.scroll_hovered));
        }
        // 0x004c5720: the tab buttons.
        for (i, &hovered) in input.tab_hovered.iter().enumerate() {
            let current = i as i32 == self.tab;
            let icon = if current || hovered { [1.0; 4] } else { [0.8, 0.8, 0.8, 1.0] };
            let frame = if hovered {
                [0.0, 1.0, 1.0, 1.0]
            } else if current {
                [0.0, 1.0, 0.0, 1.0]
            } else {
                [1.0; 4]
            };
            f.tabs.push(TabColors { icon, frame });
            if hovered {
                self.hovered_tab_button = i as i32;
            }
        }
        // 0x004c5905: the cursor's stack at the cursor (scale 0.05, no colour of its own).
        if input.held.count != 0 {
            f.held = Some(ItemModel { pos: input.cursor, scale: 0.05, color: None, item: input.held.item.clone() });
        }
        if self.hovered_index >= 0 {
            f.hovered = Some((self.hovered_tab, self.hovered_index));
        }
        f
    }
}

/// The name line of the crafting and shop cells: `World::itemName` 0x00598a50, then `" +"`
/// and [`item_level`] except for types 0xc, 0xd, 0x15, 0xb (sub type other than 0xe), 0,
/// 0x19, 0x14, 0x18, 0x17.
fn name_text(it: &[u8], item_name: &dyn Fn(&[u8]) -> String) -> String {
    let mut s = item_name(it);
    let t = it[0];
    let skip = matches!(t, 0xc | 0xd | 0x15) || (t == 0xb && it[1] != 0xe) || matches!(t, 0 | 0x19 | 0x14 | 0x18 | 0x17);
    if !skip {
        s.push_str(" +");
        s.push_str(&item_level(it).to_string());
    }
    s
}

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const COPPER: [f32; 4] = [0.8, 0.5, 0.0, 1.0];
const SILVER: [f32; 4] = [0.7, 0.7, 0.7, 1.0];
const GOLD: [f32; 4] = [1.0, 0.9, 0.0, 1.0];
const PLATINUM: [f32; 4] = [0.5, 0.2, 1.0, 1.0];

/// The widget's two `drawText` passes of one string: an outline pass (fill white, stroke
/// black, radius `stroke`) and the fill pass (`fill`, no stroke).
fn two_pass(text: &str, pos: Vec2, size: f32, stroke: f32, flags: u32, wrap: f32, fill: [f32; 4]) -> [TextDraw; 2] {
    let t = |stroke_radius, color, stroke_color| TextDraw {
        text: text.to_string(),
        pos,
        size,
        stroke_radius,
        color,
        stroke_color,
        extrusion_color: [0.0; 4],
        flags,
        wrap_width: wrap,
    };
    [t(stroke, WHITE, [0.0, 0.0, 0.0, 1.0]), t(0.0, fill, [0.0; 4])]
}

/// `0x004c7d20`: the name colour of an item: grey for type 0x15, purple (0.5, 0.1, 1) for
/// 0xd, blue (0.2, 0.5, 1) for 0x19; else by rarity (`Item+0xc`): 0 white, 1 green, 2 blue
/// (0.25, 0.25, 1), 3 purple (0.5, 0, 1), 4 yellow, other red.
pub fn rarity_color(item: &[u8]) -> [f32; 4] {
    match item[0] {
        0x15 => [0.5, 0.5, 0.5, 1.0],
        0xd => [0.5, 0.1, 1.0, 1.0],
        0x19 => [0.2, 0.5, 1.0, 1.0],
        _ => match item[0xc] {
            0 => [1.0, 1.0, 1.0, 1.0],
            1 => [0.0, 1.0, 0.0, 1.0],
            2 => [0.25, 0.25, 1.0, 1.0],
            3 => [0.5, 0.0, 1.0, 1.0],
            4 => [1.0, 1.0, 0.0, 1.0],
            _ => [1.0, 0.0, 0.0, 1.0],
        },
    }
}

/// Where the selector (`+0x16c`, `itemselector`) goes: shown at `pos` (widget-local, the
/// selected cell's corner) or hidden.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelectorPlacement {
    /// The node's Display attribute.
    pub visible: bool,
    /// `setPosition(pos)` when visible.
    pub pos: Vec2,
}

/// The inputs of one [`InventoryWidget::frame`].
pub struct FrameInput<'a> {
    /// `+0x160`: the tabs (bag: `creature+0x11dc`; crafting: `GC+0x800adc`; shop:
    /// [`ShopData::pages`]); `None` = null data, nothing is drawn.
    pub pages: Option<&'a [Vec<ItemStack>]>,
    /// data `+0x128` (bag: [`crate::ui::GameView::coins`]).
    pub coins: i32,
    /// data `+0x12c` (bag: [`crate::ui::GameView::platinum`]).
    pub platinum: i32,
    /// `(0x0062f600, 0x006291d0)`: `Gui::width` / `Gui::height` of the widget.
    pub size: Vec2,
    /// `(0x00627d50, 0x00627ce0)`: the effective local size (`Gui::local_size`).
    pub local_size: Vec2,
    /// `0x0062dc20`: the widget origin on screen (`gui.widget_world(w)` applied to (0, 0)).
    pub origin: Vec2,
    /// Engine `+0xd4/+0xd8`: the cursor on screen (`Gui::cursor`).
    pub cursor: Vec2,
    /// `Button::isHovered` 0x006294c0 of the up (+0x170), down (+0x174) and scroll (+0x178)
    /// buttons.
    pub up_hovered: bool,
    /// See [`FrameInput::up_hovered`].
    pub down_hovered: bool,
    /// See [`FrameInput::up_hovered`].
    pub scroll_hovered: bool,
    /// One entry per tab button node (`+0x17c` list order): hovered.
    pub tab_hovered: &'a [bool],
    /// `creature+0x11e8 / +0x11ec`: the cursor stack ([`crate::ui::GameView::held`]).
    pub held: &'a ItemStack,
    /// `World::itemName` 0x00598a50 ([`crate::names::item_name`]).
    pub item_name: &'a dyn Fn(&[u8]) -> String,
}

/// One `drawText` call (`FontEngine::drawText` 0x00639b30, font `resource1.dat`) in the
/// widget's local space.
#[derive(Clone, Debug, PartialEq)]
pub struct TextDraw {
    /// The text (the stream's content, trailing `\n` of `std::endl` included).
    pub text: String,
    /// Origin.
    pub pos: Vec2,
    /// Font size.
    pub size: f32,
    /// Stroke radius (0: fill pass).
    pub stroke_radius: f32,
    /// Fill colour.
    pub color: [f32; 4],
    /// Stroke colour.
    pub stroke_color: [f32; 4],
    /// Extrusion colour.
    pub extrusion_color: [f32; 4],
    /// `cw_ui::font::align` flags (2 and 0x10 here).
    pub flags: u32,
    /// Wrap width (-1: none).
    pub wrap_width: f32,
}

/// One item model drawn by `0x004758c0(x, y, GC+0x800a1c rotation, scale, item, 0)`.
#[derive(Clone, Debug, PartialEq)]
pub struct ItemModel {
    /// Screen position of the model's centre.
    pub pos: Vec2,
    /// Model scale (0.03, hovered 0.045, rings × 0.75; the cursor stack 0.05).
    pub scale: f32,
    /// The material colour set before it (`0x00448280`): black for a negative count (a
    /// recipe that cannot be made), white otherwise; `None` for the cursor stack, which
    /// keeps the colour last set.
    pub color: Option<[f32; 4]>,
    /// The item.
    pub item: Vec<u8>,
}

/// The two colours of one tab button: its icon, and its `frame` child (hovered cyan,
/// current green, else white).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TabColors {
    /// The tab node's colour (current or hovered white, else 0.8 grey).
    pub icon: [f32; 4],
    /// The `frame` child's colour.
    pub frame: [f32; 4],
}

/// What one [`InventoryWidget::frame`] produced.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InventoryFrame {
    /// The widget has data (the body ran).
    pub visible: bool,
    /// The cell models, in draw order (with the depth test on: render state 7 = 1).
    pub models: Vec<ItemModel>,
    /// The texts, in draw order.
    pub texts: Vec<TextDraw>,
    /// Up button colour (`None`: no button or the tab does not exist).
    pub up_color: Option<[f32; 4]>,
    /// Down button colour.
    pub down_color: Option<[f32; 4]>,
    /// Scroll button colour.
    pub scroll_color: Option<[f32; 4]>,
    /// The tab buttons' colours.
    pub tabs: Vec<TabColors>,
    /// The hovered cell `(tab, index)` (`+0x184/+0x188`): the item tooltip's subject.
    pub hovered: Option<(i32, i32)>,
    /// The cursor stack drawn at the cursor (every instance with data draws it).
    pub held: Option<ItemModel>,
}

/// A vendor's inventory as `0x004a2300` copies it (the creature at `GC+0x8008d8`).
#[derive(Clone, Copy, Debug)]
pub struct VendorView<'a> {
    /// `vendor+0x11dc`: the tabs.
    pub pages: &'a [Vec<ItemStack>],
    /// `vendor+0x11e8/+0x11ec`.
    pub held: &'a ItemStack,
    /// `vendor+0x1304`.
    pub coins: i32,
    /// `vendor+0x1308`.
    pub platinum: i32,
}

/// The shop's data: a `cube::Inventory` at `GC+0x800c0c` (the shop widget's `+0x160`) and
/// the buy-back list `GC+0x800d3c` (`std::list` of stacks, size at `+0x800d40`; at most 10,
/// oldest first).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShopData {
    /// `GC+0x800c0c`: the tabs (0 the vendor's, 1 buy-back).
    pub pages: Vec<Vec<ItemStack>>,
    /// `GC+0x800c18 / +0x800c1c`.
    pub held: Option<ItemStack>,
    /// `GC+0x800d34`.
    pub coins: i32,
    /// `GC+0x800d38`.
    pub platinum: i32,
    /// `GC+0x800d3c`: the sold items, oldest first.
    pub buyback: Vec<ItemStack>,
}

impl ShopData {
    /// `0x004a2300` with the shop widget on tab `shop_tab`: on tab 1 the pages are resized
    /// to two and tab 1 refilled with the buy-back list newest first; otherwise the vendor's
    /// inventory is copied (no vendor: unchanged).
    pub fn refresh(&mut self, shop_tab: i32, vendor: Option<VendorView>) {
        if shop_tab == 1 {
            self.pages.resize(2, Vec::new());
            self.pages[1].clear();
            for s in self.buyback.iter().rev() {
                self.pages[1].push(s.clone());
            }
        } else if let Some(v) = vendor {
            self.pages = v.pages.to_vec();
            self.held = Some(v.held.clone());
            self.coins = v.coins;
            self.platinum = v.platinum;
        }
    }
}

/// The three instances (`GC+0x800954`, `+0x800958`, `+0x80095c`) and the shop data.
#[derive(Clone, Debug, PartialEq)]
pub struct InventoryUi {
    /// `GC+0x800954`: the bag (type 0, `itembox` cells).
    pub bag: InventoryWidget,
    /// `GC+0x800958`: crafting (type 2, `itemselector`).
    pub crafting: InventoryWidget,
    /// `GC+0x80095c`: the shop (type 3).
    pub shop: InventoryWidget,
    /// The shop's pages and buy-back list.
    pub shop_data: ShopData,
}

impl Default for InventoryUi {
    fn default() -> Self {
        InventoryUi::new(None, None)
    }
}

impl InventoryUi {
    /// The three ctor calls of the GameController ctor 0x00459c40 (`ctor.c` 4658, 4720,
    /// 5128): `(GC, type, panel, cell template, up, down, scroll, selector)`. The shop
    /// (type 3) and crafting (type 2) cells are `wideitembox` (`GC+0x8008d0`), the bag's
    /// `itembox` (`GC+0x8008cc`); all three get the up/down/scroll buttons; only crafting
    /// gets the `itemselector` (`GC+0x8008d4`). The sizes are the templates' `(width,
    /// height)`. The tab buttons (0x004c6140) are 2 for the shop, 4 for the bag.
    pub fn new(itembox: Option<Vec2>, wideitembox: Option<Vec2>) -> Self {
        let mk = |kind, cell, selector| {
            let mut w = InventoryWidget::new(kind, cell);
            w.has_up = true;
            w.has_down = true;
            w.has_scroll = true;
            w.has_selector = selector;
            w
        };
        InventoryUi {
            bag: mk(0, itembox, false),
            crafting: mk(2, wideitembox, true),
            shop: mk(3, wideitembox, false),
            shop_data: ShopData::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(ty: u8, count: i32) -> ItemStack {
        let mut s = ItemStack::empty();
        s.count = count;
        s.item[0] = ty;
        s.item[0x10] = 1;
        s
    }

    #[test]
    fn rows_and_scroll() {
        let mut w = InventoryWidget::new(0, None);
        let size = Vec2::new(400.0, 285.0);
        // 8 columns, 5 visible rows.
        assert_eq!(w.grid(size), (8, 5));
        let pages = vec![vec![st(3, 1); 60]];
        w.update_rows(&pages, size);
        // 59 / 8 = 7; 7 - 5 + 2 = 4 positions.
        assert_eq!(w.rows, 4);
        w.scroll_down(&pages, size);
        w.scroll_down(&pages, size);
        w.scroll_down(&pages, size);
        w.scroll_down(&pages, size);
        assert_eq!(w.scroll[0], 3);
        w.scroll_up(&pages, size);
        assert_eq!(w.scroll[0], 2);
        let empty = vec![Vec::new()];
        w.update_rows(&empty, size);
        assert_eq!((w.rows, w.scroll[0]), (0, 0));
    }

    #[test]
    fn frame_hover_and_texts() {
        let mut w = InventoryWidget::new(0, None);
        w.tab = 1;
        let pages = vec![Vec::new(), vec![st(1, 3), st(3, 1), st(0, 0)]];
        let held = ItemStack::empty();
        let name = |_: &[u8]| String::from("x");
        let input = FrameInput {
            pages: Some(&pages),
            coins: 12345,
            platinum: 2,
            size: Vec2::new(400.0, 285.0),
            local_size: Vec2::new(400.0, 285.0),
            origin: Vec2::new(100.0, 50.0),
            // Cell 1: x 100 + 10 + 45 = 155, y 90.
            cursor: Vec2::new(160.0, 95.0),
            up_hovered: false,
            down_hovered: false,
            scroll_hovered: false,
            tab_hovered: &[false, true, false, false],
            held: &held,
            item_name: &name,
        };
        let f = w.frame(&input);
        assert_eq!(f.hovered, Some((1, 1)));
        // Two models (the empty cell is not drawn in the bag), the hovered one larger.
        assert_eq!(f.models.len(), 2);
        assert_eq!(f.models[1].scale, 0.045);
        assert_eq!(f.models[0].pos, Vec2::new(130.0, 110.0));
        let texts: Vec<&str> = f.texts.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(texts[0], "Items\n");
        assert!(texts.contains(&"45 C ") && texts.contains(&"23 S ") && texts.contains(&"1 G "));
        assert!(texts.contains(&"2  Platinum Coins "));
        assert!(texts.contains(&"3\n"));
        assert_eq!(w.hovered_tab_button, 1);
        assert_eq!(f.tabs[1].frame, [0.0, 1.0, 1.0, 1.0]);
        assert_eq!(f.tabs[1].icon, [1.0; 4]);
        assert_eq!(f.tabs[0].icon, [0.8, 0.8, 0.8, 1.0]);
    }

    #[test]
    fn tab_switch_selects_first_cell() {
        let mut w = InventoryWidget::new(3, None);
        let pages = vec![vec![st(3, 1)], Vec::new()];
        w.hovered_tab_button = 1;
        let (a, _) = w.switch_tab(&pages, Vec2::new(350.0, 568.0), true);
        assert_eq!(w.tab, 1);
        assert_eq!((w.selected_tab, w.selected_index), (-1, -1));
        assert_eq!(a, vec![UiAction::PlaySound { id: 0x56, volume: 1.0, pitch: 1.0 }, UiAction::RefreshShop]);
    }
}
