//! `cube::StartMenuWidget::update` `Cube.exe 0x00583320` and `cube::SystemWidget::update`
//! `0x00587710`: three text items drawn centred, the hovered one tinted. The hovered index
//! (`+0x160`, -1 = none) is what the click handlers read (`onMouseDown` 0x0047b600 for the
//! start menu).
//!
//! Both bodies return early when the font `resource1.dat` is missing (0x00639800). The
//! cursor is the engine cursor in the widget's local space (0x006294d0); an item is hovered
//! when `0 <= x < effective width` (0x00627d50) and `y` is in its band. Each item is drawn
//! twice through `FontEngine::drawText` 0x0065bc70: a black outline pass (outline width 4 for
//! the start menu, 3 for the system menu) and the fill pass, both centred on `width * 0.5`.

use glam::Vec2;

/// The hovered item (`StartMenuWidget+0x160` / `SystemWidget+0x160`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MenuState {
    /// -1 when nothing is hovered.
    pub hovered: i32,
}

impl Default for MenuState {
    fn default() -> Self {
        // ctor 0x00583270 / 0x00587660: `+0x160 = -1`.
        MenuState { hovered: -1 }
    }
}

/// One drawn item: text, baseline position (centre x, y), font size, outline width and the
/// fill colour (RGBA).
#[derive(Clone, Debug, PartialEq)]
pub struct MenuItem {
    /// The caption.
    pub text: &'static str,
    /// Centre x (`width * 0.5`) and baseline y.
    pub pos: Vec2,
    /// Font size.
    pub size: f32,
    /// Outline width of the first (black) pass.
    pub outline: f32,
    /// Fill colour: white, or `(0.2, 1, 1, 1)` when hovered.
    pub color: [f32; 4],
}

/// One menu's layout: `(text, baseline y, hover band [y0, y1))`.
struct Layout {
    items: [(&'static str, f32, f32, f32); 3],
    size: f32,
    outline: f32,
}

const START: Layout = Layout {
    // 0x00583320: "Start Game" at 30, "Options" at 80, "Exit" at 130; size 18, outline 4.
    items: [("Start Game", 30.0, 10.0, 40.0), ("Options", 80.0, 60.0, 90.0), ("Exit", 130.0, 110.0, 140.0)],
    size: 18.0,
    outline: 4.0,
};

const SYSTEM: Layout = Layout {
    // 0x00587710: "Options" at 30, "Start Menu" at 60, "Exit Game" at 90; size 14, outline 3.
    items: [("Options", 30.0, 10.0, 40.0), ("Start Menu", 60.0, 40.0, 70.0), ("Exit Game", 90.0, 70.0, 100.0)],
    size: 14.0,
    outline: 3.0,
};

fn update(layout: &Layout, state: &mut MenuState, cursor: Vec2, width: f32) -> Vec<MenuItem> {
    state.hovered = -1;
    let mut out = Vec::with_capacity(3);
    for (i, &(text, y, y0, y1)) in layout.items.iter().enumerate() {
        let mut color = [1.0, 1.0, 1.0, 1.0];
        // `0 <= x` then `x < width && y0 <= y && y < y1` (comiss pairs; NaN hovers nothing).
        if 0.0 <= cursor.x && cursor.x < width && y0 <= cursor.y && cursor.y < y1 {
            state.hovered = i as i32;
            color = [0.2, 1.0, 1.0, 1.0];
        }
        out.push(MenuItem { text, pos: Vec2::new(width * 0.5, y), size: layout.size, outline: layout.outline, color });
    }
    out
}

/// `StartMenuWidget::update` 0x00583320.
pub fn start_menu_update(state: &mut MenuState, cursor: Vec2, width: f32) -> Vec<MenuItem> {
    update(&START, state, cursor, width)
}

/// `SystemWidget::update` 0x00587710.
pub fn system_menu_update(state: &mut MenuState, cursor: Vec2, width: f32) -> Vec<MenuItem> {
    update(&SYSTEM, state, cursor, width)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hover_bands() {
        let mut s = MenuState::default();
        let items = start_menu_update(&mut s, Vec2::new(50.0, 65.0), 200.0);
        assert_eq!(s.hovered, 1);
        assert_eq!(items[1].color, [0.2, 1.0, 1.0, 1.0]);
        assert_eq!(items[0].pos, Vec2::new(100.0, 30.0));
        start_menu_update(&mut s, Vec2::new(50.0, 45.0), 200.0);
        assert_eq!(s.hovered, -1);
        start_menu_update(&mut s, Vec2::new(-1.0, 20.0), 200.0);
        assert_eq!(s.hovered, -1);
        system_menu_update(&mut s, Vec2::new(10.0, 70.0), 150.0);
        assert_eq!(s.hovered, 2);
    }
}
