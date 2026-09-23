//! The world map screen's input: the pan (`GC+0x1000e4c..+0x1000e54`, blocks) and the
//! teleporter pick (`GC+0x800dd4`, `GC+0x800ddc`, `GC+0x800de4`).
//!
//! Tier B. Every writer of the pan in `Cube.exe`:
//!
//! | Range | Here |
//! |---|---|
//! | ctor 0x0045a5b7..0x0045a5cd: `pan = 0` | `Controller::new` (`map_pan: [0.0; 3]`) |
//! | `update` 0x0048b627..0x0048b802 (map open: the middle-button drag; closed: `pan = 0`) | [`update_pan`] |
//!
//! Nothing else writes it: `onMouseMove` 0x0047ea00, `onMouseUp` 0x0047ddd0 and the wheel
//! 0x0047ef40 never touch it (the wheel only changes the zoom `GC+0x1c8`, which scales the
//! next drags). Readers: the landscape centre 0x004695da, `render` 0x004adf13 (the map view),
//! the overlay 0x004c96f1.. and 0x004ca6fa...
//!
//! The teleporter pick (the map opened from a bed, 0x00497424) is written by the overlay's
//! update (`hovered`, 0x004c96d7 / 0x004ca189, `ui::map_overlay`), [`on_release`]
//! (`onMouseUp` 0x0047df01..0x0047e008) and [`reset_when_closed`] (`update` 0x0049732f).

#![allow(clippy::assign_op_pattern)]

use crate::player::Mat4;

/// `(-1, -1)`: no zone (`GC+0x800dd4` / `+0x800ddc` when nothing is hovered or picked).
pub const NO_ZONE: [i32; 2] = [-1, -1];

/// The teleporter pick of the map screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TeleportPick {
    /// `GC+0x800dd4/+0x800dd8`: the teleporter zone under the cursor, as the last overlay
    /// update (render, 0x004ca189) left it.
    pub hovered: [i32; 2],
    /// `GC+0x800ddc/+0x800de0`: the zone the last left release picked.
    pub selected: [i32; 2],
}

impl Default for TeleportPick {
    fn default() -> Self {
        TeleportPick { hovered: NO_ZONE, selected: NO_ZONE }
    }
}

impl TeleportPick {
    /// `0x004688a0(selected, (-1, -1))` then `0x00468840(selected, hovered)`: a zone is picked
    /// and the cursor is on it again (0x0047df1b..0x0047df54, 0x0048b645..0x0048b67a).
    pub fn armed(&self) -> bool {
        self.selected != NO_ZONE && self.selected == self.hovered
    }
}

/// `update` 0x0048b6d4..0x0048b7b5, map open with the middle button (`GC+0xa`) held: the
/// cursor's motion `delta` (`Engine+0xd4 − +0xdc`, `+0xd8 − +0xe0`, pixels) becomes
/// `(dx, −dy, 0) · (4 / zoom)` (`0x00451510`, zoom the displayed `GC+0x1c4`), turned by the
/// camera's yaw `GC+0x1ac` (`mat4Identity` 0x00423e70, `mat4RotateZ` 0x00424610, `mat4MulVec3`
/// 0x00488e50) and added to the pan (0x00412850), under `GC+0x8005d0`.
pub fn drag_pan(pan: &mut [f32; 3], yaw: f32, zoom: f32, delta: [f32; 2]) {
    let mut m = Mat4::IDENTITY;
    m.rotate_z(yaw);
    // 0x0048b72c / 0x0048b750: `0x00480db0` (dy) is read first and negated (`xorps` with
    // 0x00745f70), then `0x00480d90` (dx).
    let dy = -delta[1];
    let dx = delta[0];
    // 0x0048b765: `4.0 (0x00745e60) / GC+0x1c4`.
    let s = 4.0f32 / zoom;
    let v = [dx * s, dy * s, 0.0f32 * s];
    let d = m.mul_dir(v);
    pan[0] = d[0] + pan[0];
    pan[1] = d[1] + pan[1];
    pan[2] = d[2] + pan[2];
}

/// `update` 0x0048b627..0x0048b802 (the pan part): map open (`GC+0x8006e4`) and the middle
/// button held, the drag ([`drag_pan`]); map closed, `pan = (0, 0, 0)` (0x0048b7b8..0x0048b7f6,
/// under `GC+0x8005d0`). Map open without the button leaves the pan.
pub fn update_pan(pan: &mut [f32; 3], map_open: bool, middle_held: bool, yaw: f32, zoom: f32, delta: [f32; 2]) {
    if map_open {
        if middle_held {
            drag_pan(pan, yaw, zoom, delta);
        }
    } else {
        *pan = [0.0; 3];
    }
}

/// `update` 0x0048b638..0x0048b6c6: map open, opened from a bed (`GC+0x800de4`) and the pick
/// armed: the cursor node (`GC+0x8008a8`) gets the caption `"Teleport"` (0x00701830, via
/// `Node::setText` 0x00636ad0), after the held-stack / customization caption of
/// 0x0048b4ff..0x0048b627.
pub fn teleport_caption(map_open: bool, teleport_map: bool, pick: &TeleportPick) -> Option<&'static str> {
    (map_open && teleport_map && pick.armed()).then_some("Teleport")
}

/// `onMouseUp` 0x0047df01..0x0047e008: the left button (`button == 0`) released with the map
/// open. Opened from a bed with the pick armed ([`TeleportPick::armed`]): returns the player's
/// new position (`creature+0x10`, copied by 0x0042c5b0 under the world lock `World+0x8000c0`):
/// the picked zone's centre `((z · 256 + 128) << 16)` per axis (32-bit product, sign-extended,
/// 0x0047df65..0x0047df8e) and `z = 0`; clears both zones and closes the map (`GC+0x8006e4 =
/// 0`; `GC+0x800de4` stays until [`reset_when_closed`]). Otherwise the hovered zone becomes the
/// pick (0x0047dff6..0x0047e008), also with the map opened normally.
pub fn on_release(button: i32, map_open: &mut bool, teleport_map: bool, pick: &mut TeleportPick) -> Option<[i64; 3]> {
    if button != 0 || !*map_open {
        return None;
    }
    if teleport_map && pick.armed() {
        let fx = i64::from(pick.selected[0].wrapping_shl(8).wrapping_add(0x80)) << 16;
        let fy = i64::from(pick.selected[1].wrapping_shl(8).wrapping_add(0x80)) << 16;
        *pick = TeleportPick::default();
        *map_open = false;
        return Some([fx, fy, 0]);
    }
    pick.selected = pick.hovered;
    None
}

/// `update` 0x0049732f..0x0049735f (after the speech bubbles, before the R key): with the map
/// closed, `GC+0x800de4 = 0` and the pick `GC+0x800ddc = (-1, -1)` (0x00411df0); the hovered
/// zone is left.
pub fn reset_when_closed(map_open: bool, teleport_map: &mut bool, pick: &mut TeleportPick) {
    if !map_open {
        *teleport_map = false;
        pick.selected = NO_ZONE;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_at_zero_yaw_moves_by_four_over_zoom() {
        let mut pan = [1.0, 2.0, 3.0];
        drag_pan(&mut pan, 0.0, 2.0, [10.0, 5.0]);
        // (10, -5, 0) * 2 added.
        assert_eq!(pan, [21.0, -8.0, 3.0]);
    }

    #[test]
    fn drag_turns_with_the_yaw() {
        let mut pan = [0.0; 3];
        drag_pan(&mut pan, 90.0, 4.0, [1.0, 0.0]);
        // Rz(90): row0 = (c, s), row1 = (-s, c); out.x = c*x - s*y = ~0, out.y = s*x = 1.
        assert!(pan[0].abs() < 1e-6, "{pan:?}");
        assert!((pan[1] - 1.0).abs() < 1e-6, "{pan:?}");
        assert_eq!(pan[2], 0.0);
    }

    #[test]
    fn drag_matches_the_original_float_shape() {
        let mut pan = [0.5, -0.25, 0.0];
        let (yaw, zoom, d) = (37.0f32, 0.7f32, [3.0f32, -2.0f32]);
        drag_pan(&mut pan, yaw, zoom, d);
        let mut m = Mat4::IDENTITY;
        m.rotate_z(yaw);
        let s = 4.0f32 / zoom;
        let v = [d[0] * s, (-d[1]) * s, 0.0];
        let m = m.0;
        let x = m[4] * v[1] + v[0] * m[0] + m[8] * v[2];
        let y = m[1] * v[0] + m[5] * v[1] + m[9] * v[2];
        assert_eq!(pan, [x + 0.5, y + -0.25, m[2] * v[0] + m[6] * v[1] + m[10] * v[2]]);
    }

    #[test]
    fn update_pan_needs_the_middle_button_and_resets_when_closed() {
        let mut pan = [5.0, 6.0, 7.0];
        update_pan(&mut pan, true, false, 0.0, 1.0, [100.0, 100.0]);
        assert_eq!(pan, [5.0, 6.0, 7.0]);
        update_pan(&mut pan, true, true, 0.0, 1.0, [1.0, 1.0]);
        assert_eq!(pan, [9.0, 2.0, 7.0]);
        update_pan(&mut pan, false, true, 0.0, 1.0, [1.0, 1.0]);
        assert_eq!(pan, [0.0; 3]);
    }

    #[test]
    fn first_release_picks_the_hovered_zone() {
        let mut open = true;
        let mut pick = TeleportPick { hovered: [3, 4], selected: NO_ZONE };
        assert_eq!(on_release(0, &mut open, true, &mut pick), None);
        assert_eq!(pick.selected, [3, 4]);
        assert!(open);
        assert!(pick.armed());
        assert_eq!(teleport_caption(true, true, &pick), Some("Teleport"));
        assert_eq!(teleport_caption(true, false, &pick), None);
    }

    #[test]
    fn second_release_on_the_same_zone_teleports_and_closes() {
        let mut open = true;
        let mut pick = TeleportPick { hovered: [3, -2], selected: [3, -2] };
        let p = on_release(0, &mut open, true, &mut pick);
        assert_eq!(p, Some([(3 * 256 + 128) << 16, (-2 * 256 + 128) << 16, 0]));
        assert!(!open);
        assert_eq!(pick, TeleportPick::default());
    }

    #[test]
    fn release_elsewhere_or_off_zone_repicks() {
        let mut open = true;
        let mut pick = TeleportPick { hovered: NO_ZONE, selected: [3, 4] };
        assert_eq!(on_release(0, &mut open, true, &mut pick), None);
        assert_eq!(pick.selected, NO_ZONE);
        // A picked (-1, -1) never arms.
        assert!(!pick.armed());
    }

    #[test]
    fn release_without_bed_mode_only_copies_the_pick() {
        let mut open = true;
        let mut pick = TeleportPick { hovered: [1, 1], selected: [1, 1] };
        assert_eq!(on_release(0, &mut open, false, &mut pick), None);
        assert!(open);
        assert_eq!(pick.selected, [1, 1]);
    }

    #[test]
    fn release_ignores_other_buttons_and_a_closed_map() {
        let mut open = true;
        let mut pick = TeleportPick { hovered: [1, 1], selected: [1, 1] };
        assert_eq!(on_release(1, &mut open, true, &mut pick), None);
        assert!(open);
        let mut closed = false;
        let mut pick2 = TeleportPick { hovered: [2, 2], selected: NO_ZONE };
        assert_eq!(on_release(0, &mut closed, true, &mut pick2), None);
        assert_eq!(pick2.selected, NO_ZONE);
    }

    #[test]
    fn zone_centre_uses_a_32_bit_product() {
        // 0x0047df65: `shl 8; sub -0x80; cdq`: the 32-bit value wraps before the widening.
        let mut open = true;
        let z = 0x0080_0000;
        let mut pick = TeleportPick { hovered: [z, 0], selected: [z, 0] };
        let p = on_release(0, &mut open, true, &mut pick).unwrap();
        assert_eq!(p[0], i64::from(z.wrapping_shl(8).wrapping_add(0x80)) << 16);
        assert!(p[0] < 0);
    }

    #[test]
    fn closed_map_clears_bed_mode_and_the_pick() {
        let mut t = true;
        let mut pick = TeleportPick { hovered: [1, 2], selected: [3, 4] };
        reset_when_closed(true, &mut t, &mut pick);
        assert!(t);
        assert_eq!(pick.selected, [3, 4]);
        reset_when_closed(false, &mut t, &mut pick);
        assert!(!t);
        assert_eq!(pick, TeleportPick { hovered: [1, 2], selected: NO_ZONE });
    }
}
