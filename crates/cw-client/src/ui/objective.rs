//! `cube::ObjectiveWidget` (vtable 0x007030dc, ctor `Cube.exe 0x004ce180`, 0x170 bytes):
//! an empty list in this build; only its node's visibility is driven.
//!
//! | Field | Offset |
//! |---|---|
//! | `std::vector` (begin, end, capacity; element type unknown) | +0x160..+0x168 |
//! | GameController* (ctor `param_4`) | +0x16c |
//!
//! The GameController ctor creates an unnamed node for it (`GC+0x800944`, 0x00461d05, no
//! `.plx` content) and the widget (`GC+0x800948`, 0x00461d50); `onResize` 0x00482a40 puts the
//! node at `(20, 170)` (0x004836b5, cw-ui `game_controller_on_resize`). Only slot 0 (the
//! dtor 0x004ce240, which frees the vector buffer) is overridden. `GameController::update`
//! 0x00488ee0 touches it twice:
//!
//! * 0x004908ae: `vector.clear()` (0x0044be20: `end = begin`) every frame;
//! * 0x0049845f: `node.setVisible(!worldName.empty())` (0x00411a90 with the negated
//!   `0x00477220`, the `std::string` size test, on `World+0x94`, the world name, saved in the
//!   stack slot `[esp+0x6c]` at 0x0048c13f).
//!
//! Nothing ever appends to the vector (a scan of `.text` finds the immediate 0x800948 only
//! in the ctor and at 0x004908ae, and the widget has no draw override), so the widget draws
//! nothing. The quest objective text of the HUD is the `quest-tag.plx` node (`GC+0x800868`)
//! filled at 0x00490753 through the joiner 0x00477fa0 ([`super::tooltip::join_words`]), not
//! this widget.

use glam::Vec2;

/// The widget's fields.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ObjectiveWidget {
    /// +0x160: the entries (never filled; kept for the clear of 0x004908ae).
    pub entries: Vec<u32>,
}

/// What the widget shows in a frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ObjectiveFrame {
    /// The node's visibility (0x0049845f).
    pub visible: bool,
    /// The node position `onResize` sets (0x004836cd: `(20, 170)`).
    pub position: Vec2,
    /// Texts drawn (none).
    pub texts: Vec<String>,
}

/// The node position of `onResize` 0x004836cd (`0x41a00000`, `0x432a0000`).
pub const POSITION: Vec2 = Vec2::new(20.0, 170.0);

/// The two steps of `update`: clear the vector (0x004908ae), then show the node when a world
/// is loaded (`world_name` is `World+0x94`, 0x0049845f). The caller writes `visible` to
/// `GcMembers::objective_node`.
pub fn frame(state: &mut ObjectiveWidget, world_name: &str) -> ObjectiveFrame {
    state.entries.clear();
    ObjectiveFrame { visible: !world_name.is_empty(), position: POSITION, texts: Vec::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visibility_follows_world_name() {
        let mut w = ObjectiveWidget { entries: vec![1, 2] };
        assert!(!frame(&mut w, "").visible);
        assert!(w.entries.is_empty());
        let f = frame(&mut w, "Cubeville");
        assert!(f.visible);
        assert_eq!(f.position, Vec2::new(20.0, 170.0));
    }
}
