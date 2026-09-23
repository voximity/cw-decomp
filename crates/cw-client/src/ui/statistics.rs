//! `cube::StatisticsWidget` (vtable 0x0071a22c, ctor `Cube.exe 0x00583b40`, 0x164 bytes):
//! a placeholder in this build.
//!
//! The GameController ctor creates an unnamed node for it (`GC+0x80094c`, `createNode`
//! 0x0064f4e0 at 0x00461d9a, no `.plx` content) and the widget (`GC+0x800950`,
//! 0x00461ddf). `onResize` 0x00482a40 places the node at `(20, 470)` (0x004836ea, cw-ui
//! `game_controller_on_resize`). Its only overridden slot, update (slot 1, 0x00583be0), is
//! an empty body (`mov [ebp-4], 0; ret`: a stripped debug routine), the base draw has
//! nothing to draw, and no code other than the ctor and `onResize` references the node or
//! the widget (a scan of `.text` for the immediates 0x80094c / 0x800950). The node's
//! visibility is never written after construction.
//!
//! Tier B: the frame is always empty.

use glam::Vec2;

/// The widget's fields.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StatisticsWidget {
    // +0x160: the GameController pointer the ctor stores (`param_4`); never read.
}

/// What the widget shows in a frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatisticsFrame {
    /// The node position `onResize` sets (0x00483702: `(20, 470)`).
    pub position: Vec2,
    /// Texts drawn (none: slot 1 0x00583be0 is empty).
    pub texts: Vec<String>,
}

/// The node position of `onResize` 0x00483702 (`0x41a00000`, `0x43eb0000`).
pub const POSITION: Vec2 = Vec2::new(20.0, 470.0);

/// `StatisticsWidget::update` 0x00583be0: does nothing.
pub fn frame(_state: &StatisticsWidget) -> StatisticsFrame {
    StatisticsFrame { position: POSITION, texts: Vec::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty() {
        let f = frame(&StatisticsWidget::default());
        assert!(f.texts.is_empty());
        assert_eq!(f.position, Vec2::new(20.0, 470.0));
    }
}
