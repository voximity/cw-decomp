//! `cube::OptionsWidget` (vtable 0x0070330c, ctor 0x004cf3c0, 0x208 bytes): eleven option
//! rows with left/right arrows and the Apply / OK / Cancel buttons.
//!
//! | Field | Offset |
//! |---|---|
//! | display modes `vector<pair<int,int>>` | +0x160 |
//! | GameController* | +0x16c |
//! | 22 arrows (left/right per row) | +0x170..+0x1c4 |
//! | Apply / OK / Cancel (110x15 clones of `button2`) | +0x1c8 / +0x1cc / +0x1d0 |
//! | the edited options block (12 ints) | +0x1d4..+0x200 |
//! | display mode index | +0x204 |
//!
//! The arrows run [`crate::options::Options::step`] on the edited copy (the resolution pair
//! steps the mode index: 0x004d4cb0 / 0x004d4d20). Apply (0x004d4470) calls
//! `GameController::applyOptions` 0x0046f390 with the copy; OK (0x004d4650) applies and hides
//! the panel (the parent widget's node, 0x0062b400); Cancel (0x004d4510) only hides it.

use crate::options::{OptionKey, Options};

use super::UiAction;

/// The rows in drawing order with the key their arrows step (`None`: resolution).
pub const ROWS: [(&str, Option<OptionKey>); 11] = [
    ("Mode", Some(OptionKey::Fullscreen)),
    ("Resolution", None),
    ("Anti-aliasing", Some(OptionKey::AntiAliasing)),
    ("Render Distance", Some(OptionKey::RenderDistance)),
    ("Sound FX Volume", Some(OptionKey::SoundVolume)),
    ("Music Volume", Some(OptionKey::MusicVolume)),
    ("Camera Speed", Some(OptionKey::CameraSpeed)),
    ("Camera Smoothness", Some(OptionKey::CameraSmoothness)),
    ("Invert Y Axis", Some(OptionKey::InvertY)),
    ("FPS Limit", Some(OptionKey::MinTimeStep)),
    ("Language", Some(OptionKey::Language)),
];

/// The widget's state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OptionsMenu {
    /// +0x160.
    pub modes: Vec<(i32, i32)>,
    /// +0x1d4.
    pub edited: Options,
    /// +0x204.
    pub mode_index: u32,
}

/// The three buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptionsButton {
    /// 0x004d4470.
    Apply,
    /// 0x004d4650.
    Ok,
    /// 0x004d4510.
    Cancel,
}

impl OptionsMenu {
    /// 0x004d4de0 (called by the GameController ctor): copies the block, and the mode index
    /// becomes the *last* mode equal to the block's resolution (0 when none is).
    pub fn load(&mut self, options: &Options, modes: &[(i32, i32)]) {
        self.modes = modes.to_vec();
        self.edited = *options;
        self.mode_index = 0;
        for (i, &(w, h)) in self.modes.iter().enumerate() {
            if w == options.resolution_x && h == options.resolution_y {
                self.mode_index = i as u32;
            }
        }
        self.sync_resolution();
    }

    /// `resolution = modes[index % n]` (the tail of 0x004d4cb0 / 0x004d4d20 / 0x004d4de0).
    fn sync_resolution(&mut self) {
        if !self.modes.is_empty() {
            let (w, h) = self.modes[(self.mode_index % self.modes.len() as u32) as usize];
            self.edited.resolution_x = w;
            self.edited.resolution_y = h;
        }
    }

    /// One arrow of row `row` (index into [`ROWS`]); `right` is the right arrow.
    pub fn step(&mut self, row: usize, right: bool) {
        match ROWS[row].1 {
            Some(key) => self.edited.step(key, right),
            None => {
                if right {
                    // 0x004d4d20: +1, capped at n - 1 (signed compare).
                    let n = self.modes.len() as i32;
                    let mut i = self.mode_index as i32 + 1;
                    if n <= i {
                        i = n - 1;
                    }
                    self.mode_index = i as u32;
                } else {
                    // 0x004d4cb0: -1, floored at 0.
                    let i = self.mode_index as i32 - 1;
                    self.mode_index = if i < 0 { 0 } else { i as u32 };
                }
                self.sync_resolution();
            }
        }
    }

    /// Apply / OK / Cancel. Returns the actions and whether the panel hides.
    pub fn press(&self, b: OptionsButton) -> (Vec<UiAction>, bool) {
        match b {
            OptionsButton::Apply => (vec![UiAction::ApplyOptions(self.edited)], false),
            OptionsButton::Ok => (vec![UiAction::ApplyOptions(self.edited)], true),
            OptionsButton::Cancel => (Vec::new(), true),
        }
    }

    /// The value texts of `OptionsWidget::update` 0x004d0230, row by row.
    pub fn values(&self) -> [String; 11] {
        let o = &self.edited;
        let pct = |v: i32| format!("{v}%");
        [
            // `+0x1d4 == 0` → "Windowed".
            if o.fullscreen == 0 { "Windowed" } else { "Fullscreen" }.to_string(),
            // `w << L" x " << h` of the current mode (nothing when the list is empty).
            if self.modes.is_empty() {
                String::new()
            } else {
                let (w, h) = self.modes[self.mode_index as usize % self.modes.len()];
                format!("{w} x {h}")
            },
            // `< 1` → "Disabled", else `2n << L"x"`.
            if o.anti_aliasing < 1 { "Disabled".to_string() } else { format!("{}x", o.anti_aliasing * 2) },
            pct(o.render_distance),
            pct(o.sound_volume),
            pct(o.music_volume),
            pct(o.camera_speed),
            pct(o.camera_smoothness),
            // Narrow "No" / "Yes".
            if o.invert_y == 0 { "No" } else { "Yes" }.to_string(),
            o.fps_limit_label(),
            o.language.to_string(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_and_resolution() {
        let mut m = OptionsMenu::default();
        let o = Options { resolution_x: 1280, resolution_y: 720, ..Options::default() };
        m.load(&o, &[(800, 600), (1280, 720), (1920, 1080)]);
        assert_eq!(m.mode_index, 1);
        m.step(1, true);
        m.step(1, true);
        assert_eq!((m.edited.resolution_x, m.edited.resolution_y), (1920, 1080));
        m.step(1, false);
        m.step(1, false);
        m.step(1, false);
        assert_eq!(m.mode_index, 0);
        m.step(2, true);
        let v = m.values();
        assert_eq!(v[0], "Windowed");
        assert_eq!(v[1], "800 x 600");
        assert_eq!(v[2], "2x");
        assert_eq!(v[3], "50%");
        assert_eq!(v[9], "111 fps");
        let (a, hide) = m.press(OptionsButton::Ok);
        assert!(hide && matches!(a[0], UiAction::ApplyOptions(_)));
    }
}
