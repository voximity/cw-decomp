//! The options block and `options.cfg` (Cube.exe).
//!
//! The block is twelve `i32`s, a global at `0x0076b1d8` mirrored at `GameController+0x170`
//! (`analysis/notes/client-classes.md`, "Options block"). Its constructor `0x004ce660` sets the
//! defaults, `WinMain` 0x004c8ae0 overwrites the resolution with the screen size
//! (`GetSystemMetrics(0/1)`) and then calls [`Options::load`] (`0x004ce6e0`). The options menu
//! edits a copy of the block at `OptionsWidget+0x1d4` with the button callbacks ported in
//! [`Options::step`] and applies it through `GameController::applyOptions` `0x0046f390`, which
//! copies the block into the controller and calls [`Options::save`] (`0x004cef80`).
//!
//! Tier A: the file format. `load` is `std::ifstream` token extraction (`>> std::string`
//! then `>> int` when the token names a key); `save` writes `key value` lines through a
//! text-mode `std::ofstream`, so every line ends in CR LF on the original's platform.

#![allow(dead_code)]

use std::path::Path;

/// The file both functions use, relative to the working directory.
pub const OPTIONS_FILE: &str = "options.cfg";

/// The options block, `0x0076b1d8` (`GameController+0x170`). Field order is the memory
/// order; the file order differs (`language` is written before `invertY`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// `[0]` `fullscreen`: 0 windowed, 1 fullscreen (the menu toggles it).
    pub fullscreen: i32,
    /// `[1]` `resolutionX`.
    pub resolution_x: i32,
    /// `[2]` `resolutionY`.
    pub resolution_y: i32,
    /// `[3]` `antiAliasing`: 0..2; the D3D multisample type is `antiAliasing * 2`
    /// (`resetDevice` 0x004c8940), shown as "Disabled" or `2n` by the menu.
    pub anti_aliasing: i32,
    /// `[4]` `renderDistance`: 25..100 in steps of 5 from the menu.
    pub render_distance: i32,
    /// `[5]` `soundVolume`: 0..100 (percent), the factor of every sound (`playSound`
    /// 0x00484350: `soundVolume * gain * 0.01`).
    pub sound_volume: i32,
    /// `[6]` `musicVolume`: 0..100; only feeds `setMusicVolumes` (there is no music).
    pub music_volume: i32,
    /// `[7]` `cameraSpeed`: 10..100; mouse look is `delta * cameraSpeed * 0.005`.
    pub camera_speed: i32,
    /// `[8]` `cameraSmoothness`: 0..100.
    pub camera_smoothness: i32,
    /// `[9]` `invertY`: 0 or 1.
    pub invert_y: i32,
    /// `[10]` `language`: an index the menu steps without bounds.
    pub language: i32,
    /// `[11]` `minTimeStep`: the "FPS Limit" option, the minimum frame time in milliseconds
    /// (0..50). `WinMain`'s loop sleeps `minTimeStep - frameTime` when the frame took less
    /// (0x004c9057..); the menu shows `1000 / minTimeStep` fps, or "none" for 0.
    pub min_time_step: i32,
}

impl Default for Options {
    /// `Options::Options` 0x004ce660.
    fn default() -> Self {
        Options {
            fullscreen: 0,
            resolution_x: 0,
            resolution_y: 0,
            anti_aliasing: 0,
            render_distance: 0x32,
            sound_volume: 100,
            music_volume: 100,
            camera_speed: 0x32,
            camera_smoothness: 0x50,
            invert_y: 0,
            language: 0,
            min_time_step: 9,
        }
    }
}

/// The keys `load` compares each token against, in its comparison order (0x004ce6e0), with the
/// index of the field each one sets. `language` (index 10) is tested before `invertY` (9).
pub const LOAD_KEYS: [(&str, usize); 12] = [
    ("fullscreen", 0),
    ("resolutionX", 1),
    ("resolutionY", 2),
    ("antiAliasing", 3),
    ("renderDistance", 4),
    ("soundVolume", 5),
    ("musicVolume", 6),
    ("cameraSpeed", 7),
    ("cameraSmoothness", 8),
    ("language", 10),
    ("invertY", 9),
    ("minTimeStep", 11),
];

/// Which option a pair of left/right buttons of the options menu changes (`OptionsWidget`
/// constructor 0x004cf3c0 and its callbacks 0x004d4430..0x004d4dc0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionKey {
    Fullscreen,
    AntiAliasing,
    RenderDistance,
    SoundVolume,
    MusicVolume,
    CameraSpeed,
    CameraSmoothness,
    InvertY,
    Language,
    MinTimeStep,
}

impl Options {
    /// The block as the original's twelve ints.
    pub fn to_array(self) -> [i32; 12] {
        [
            self.fullscreen,
            self.resolution_x,
            self.resolution_y,
            self.anti_aliasing,
            self.render_distance,
            self.sound_volume,
            self.music_volume,
            self.camera_speed,
            self.camera_smoothness,
            self.invert_y,
            self.language,
            self.min_time_step,
        ]
    }

    pub fn from_array(a: [i32; 12]) -> Options {
        Options {
            fullscreen: a[0],
            resolution_x: a[1],
            resolution_y: a[2],
            anti_aliasing: a[3],
            render_distance: a[4],
            sound_volume: a[5],
            music_volume: a[6],
            camera_speed: a[7],
            camera_smoothness: a[8],
            invert_y: a[9],
            language: a[10],
            min_time_step: a[11],
        }
    }

    /// `WinMain` 0x004c8ae0: the constructor's defaults, the screen size as the resolution,
    /// then [`Options::load`] from `options.cfg` in the working directory.
    pub fn startup(screen_w: i32, screen_h: i32) -> Options {
        let mut o = Options { resolution_x: screen_w, resolution_y: screen_h, ..Options::default() };
        o.load(OPTIONS_FILE);
        o
    }

    /// `Options::load` 0x004ce6e0: reads `path` over the current values. A missing file leaves
    /// everything as it is.
    pub fn load(&mut self, path: impl AsRef<Path>) {
        if let Ok(bytes) = std::fs::read(path) {
            self.load_from_bytes(&bytes);
        }
    }

    /// The extraction loop of 0x004ce6e0 on the file's bytes:
    ///
    /// ```text
    /// while (stream.good()) {
    ///     std::string key; stream >> key;          // 0x004ce2a0
    ///     if (stream.fail()) break;
    ///     if (key == "fullscreen") stream >> v[0];  // every comparison runs, no else
    ///     ...
    /// }
    /// ```
    ///
    /// A token that names no key is skipped, so an unknown key's value is read as the next
    /// key. A value that does not parse as an integer fails the stream and ends the loop.
    pub fn load_from_bytes(&mut self, bytes: &[u8]) {
        let mut s = Extract { bytes, pos: 0, fail: false };
        let mut v = self.to_array();
        while !s.fail {
            let Some(key) = s.token() else { break };
            for (name, index) in LOAD_KEYS {
                if key == name.as_bytes() {
                    s.int(&mut v[index]);
                }
            }
        }
        *self = Options::from_array(v);
    }

    /// `Options::save` 0x004cef80: the text `options.cfg` holds after a save.
    pub fn to_file_bytes(&self) -> Vec<u8> {
        // Write order of 0x004cef80: language before invertY, like the load comparisons.
        let lines: [(&str, i32); 12] = [
            ("fullscreen", self.fullscreen),
            ("resolutionX", self.resolution_x),
            ("resolutionY", self.resolution_y),
            ("antiAliasing", self.anti_aliasing),
            ("renderDistance", self.render_distance),
            ("soundVolume", self.sound_volume),
            ("musicVolume", self.music_volume),
            ("cameraSpeed", self.camera_speed),
            ("cameraSmoothness", self.camera_smoothness),
            ("language", self.language),
            ("invertY", self.invert_y),
            ("minTimeStep", self.min_time_step),
        ];
        let mut out = Vec::new();
        for (key, value) in lines {
            // `os << "key " << value << std::endl`; the text-mode stream turns '\n' into CR LF.
            out.extend_from_slice(key.as_bytes());
            out.push(b' ');
            out.extend_from_slice(value.to_string().as_bytes());
            out.extend_from_slice(b"\r\n");
        }
        out
    }

    /// `Options::save` 0x004cef80: rewrites `path` (open mode `ios::out`, truncating).
    pub fn save(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        std::fs::write(path, self.to_file_bytes())
    }

    /// One press of an options-menu arrow on the menu's copy of the block
    /// (`OptionsWidget+0x1d4`); `right` is the right-hand button. From the callbacks the
    /// constructor 0x004cf3c0 wires, pairs in widget order:
    ///
    /// | option | left | right |
    /// |---|---|---|
    /// | fullscreen | 0x004d4570 toggle (also logs a line) | 0x004d45b0 toggle |
    /// | antiAliasing | 0x004d4430 −1, floor 0 | 0x004d4450 +1, cap 2 |
    /// | renderDistance | 0x004d4690 −5, floor 25 | 0x004d46b0 +5, cap 100 |
    /// | soundVolume | 0x004d4da0 −10, floor 0 | 0x004d4dc0 +10, cap 100 |
    /// | musicVolume | 0x004d4610 −10, floor 0 | 0x004d4630 +10, cap 100 |
    /// | cameraSpeed | 0x004d44d0 −5, floor 10 | 0x004d44f0 +5, cap 100 |
    /// | cameraSmoothness | 0x004d4490 −5, floor 0 | 0x004d44b0 +5, cap 100 |
    /// | invertY | 0x004d45d0 toggle | 0x004d45d0 toggle |
    /// | minTimeStep (FPS Limit) | 0x004d4530 +1, cap 50 | 0x004d4550 −1, floor 0 |
    /// | language | 0x004d45f0 −1 | 0x004d4600 +1 (no bounds) |
    ///
    /// The FPS Limit's left arrow raises the time step, which lowers the frame rate shown.
    /// The resolution pair (0x004d4cb0 / 0x004d4d20) steps an index into the display-mode list
    /// and is left to the widget, which owns that list.
    pub fn step(&mut self, key: OptionKey, right: bool) {
        // The floors are `cmovs` (below zero) or `cmovl` (below the bound), the caps `cmovg`.
        fn down(v: &mut i32, by: i32, floor: i32) {
            *v = v.wrapping_sub(by);
            if *v < floor {
                *v = floor;
            }
        }
        fn up(v: &mut i32, by: i32, cap: i32) {
            *v = v.wrapping_add(by);
            if *v > cap {
                *v = cap;
            }
        }
        // `(v + 1) % 2` with the sign of the dividend (0x004d45b0).
        fn toggle_mod(v: &mut i32) {
            *v = v.wrapping_add(1) % 2;
        }
        match (key, right) {
            (OptionKey::Fullscreen, _) => toggle_mod(&mut self.fullscreen),
            (OptionKey::AntiAliasing, false) => down(&mut self.anti_aliasing, 1, 0),
            (OptionKey::AntiAliasing, true) => up(&mut self.anti_aliasing, 1, 2),
            (OptionKey::RenderDistance, false) => down(&mut self.render_distance, 5, 25),
            (OptionKey::RenderDistance, true) => up(&mut self.render_distance, 5, 100),
            (OptionKey::SoundVolume, false) => down(&mut self.sound_volume, 10, 0),
            (OptionKey::SoundVolume, true) => up(&mut self.sound_volume, 10, 100),
            (OptionKey::MusicVolume, false) => down(&mut self.music_volume, 10, 0),
            (OptionKey::MusicVolume, true) => up(&mut self.music_volume, 10, 100),
            (OptionKey::CameraSpeed, false) => down(&mut self.camera_speed, 5, 10),
            (OptionKey::CameraSpeed, true) => up(&mut self.camera_speed, 5, 100),
            (OptionKey::CameraSmoothness, false) => down(&mut self.camera_smoothness, 5, 0),
            (OptionKey::CameraSmoothness, true) => up(&mut self.camera_smoothness, 5, 100),
            // 0x004d45d0: `v = (v == 0)`.
            (OptionKey::InvertY, _) => self.invert_y = (self.invert_y == 0) as i32,
            (OptionKey::MinTimeStep, false) => up(&mut self.min_time_step, 1, 50),
            (OptionKey::MinTimeStep, true) => down(&mut self.min_time_step, 1, 0),
            (OptionKey::Language, false) => self.language = self.language.wrapping_sub(1),
            (OptionKey::Language, true) => self.language = self.language.wrapping_add(1),
        }
    }

    /// The "FPS Limit" value text of `OptionsWidget::update` 0x004d0230 (0x004d3160..): "none"
    /// (0x00703530) when `minTimeStep <= 0`, else `1000 / minTimeStep` (integer division)
    /// followed by " fps" (0x00703538).
    pub fn fps_limit_label(&self) -> String {
        if self.min_time_step <= 0 {
            "none".to_string()
        } else {
            format!("{} fps", 1000 / self.min_time_step)
        }
    }

    /// The frame limiter of `WinMain`'s loop (after `frame()` at 0x004c9057..): the
    /// milliseconds to sleep after a frame that took `frame_ms` (`timeGetTime` difference,
    /// compared signed), or 0.
    pub fn frame_sleep_ms(&self, frame_ms: i32) -> u32 {
        if frame_ms < self.min_time_step { (self.min_time_step - frame_ms) as u32 } else { 0 }
    }
}

/// `std::basic_istream<char>` extraction over a byte buffer, as far as `load` uses it.
struct Extract<'a> {
    bytes: &'a [u8],
    pos: usize,
    fail: bool,
}

impl<'a> Extract<'a> {
    /// `isspace` in the "C" locale.
    fn is_space(b: u8) -> bool {
        matches!(b, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r')
    }

    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len() && Self::is_space(self.bytes[self.pos]) {
            self.pos += 1;
        }
    }

    /// `operator>>(istream&, std::string&)`: skip white space, take everything up to the next
    /// white space; fails (and returns `None`) when nothing is left.
    fn token(&mut self) -> Option<&'a [u8]> {
        if self.fail {
            return None;
        }
        self.skip_ws();
        let start = self.pos;
        while self.pos < self.bytes.len() && !Self::is_space(self.bytes[self.pos]) {
            self.pos += 1;
        }
        if self.pos == start {
            self.fail = true;
            return None;
        }
        Some(&self.bytes[start..self.pos])
    }

    /// `istream::operator>>(int&)` through `num_get` (decimal): skip white space, an optional
    /// sign, then digits. Without a digit, or on overflow, the stream fails; the VS2012
    /// library leaves the target unchanged then (see the report's assumptions).
    fn int(&mut self, out: &mut i32) {
        if self.fail {
            return;
        }
        self.skip_ws();
        let mut p = self.pos;
        let mut neg = false;
        if p < self.bytes.len() && (self.bytes[p] == b'+' || self.bytes[p] == b'-') {
            neg = self.bytes[p] == b'-';
            p += 1;
        }
        let digits_start = p;
        let mut value: i64 = 0;
        let mut overflow = false;
        while p < self.bytes.len() && self.bytes[p].is_ascii_digit() {
            value = value * 10 + i64::from(self.bytes[p] - b'0');
            if value > 1 << 32 {
                overflow = true;
                value = 1 << 32;
            }
            p += 1;
        }
        self.pos = p;
        if p == digits_start {
            self.fail = true;
            return;
        }
        let v = if neg { -value } else { value };
        if overflow || v < i64::from(i32::MIN) || v > i64::from(i32::MAX) {
            self.fail = true;
            return;
        }
        *out = v as i32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_constructor() {
        assert_eq!(Options::default().to_array(), [0, 0, 0, 0, 50, 100, 100, 50, 80, 0, 0, 9]);
        assert_eq!(Options::default().fps_limit_label(), "111 fps");
    }

    #[test]
    fn save_then_load_round_trips() {
        let o = Options {
            fullscreen: 1,
            resolution_x: 1920,
            resolution_y: 1080,
            anti_aliasing: 2,
            render_distance: 75,
            sound_volume: 30,
            music_volume: 0,
            camera_speed: 15,
            camera_smoothness: 5,
            invert_y: 1,
            language: -3,
            min_time_step: 0,
        };
        let bytes = o.to_file_bytes();
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(text.starts_with("fullscreen 1\r\nresolutionX 1920\r\n"));
        assert!(text.contains("language -3\r\ninvertY 1\r\nminTimeStep 0\r\n"));
        let mut back = Options::default();
        back.load_from_bytes(&bytes);
        assert_eq!(back, o);

        let dir = std::env::temp_dir().join(format!("cw-client-options-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(OPTIONS_FILE);
        o.save(&path).unwrap();
        let mut from_disk = Options::default();
        from_disk.load(&path);
        assert_eq!(from_disk, o);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_follows_the_stream_rules() {
        // An unknown key's value becomes the next key; LF-only files load too.
        let mut o = Options::default();
        o.load_from_bytes(b"bogus 7 soundVolume 20\nlanguage 4");
        assert_eq!(o.sound_volume, 20);
        assert_eq!(o.language, 4);
        // A bad value fails the stream: nothing after it is read.
        let mut o = Options::default();
        o.load_from_bytes(b"cameraSpeed x minTimeStep 1");
        assert_eq!(o.camera_speed, 50);
        assert_eq!(o.min_time_step, 9);
        // "12abc": 12 is taken, "abc" is read as the next key.
        let mut o = Options::default();
        o.load_from_bytes(b"renderDistance 12abc invertY 1");
        assert_eq!((o.render_distance, o.invert_y), (12, 1));
        // A missing file leaves the block alone.
        let mut o = Options::default();
        o.load("this/file/does/not/exist.cfg");
        assert_eq!(o, Options::default());
    }

    #[test]
    fn menu_steps_and_fps_label() {
        let mut o = Options { min_time_step: 49, ..Options::default() };
        o.step(OptionKey::MinTimeStep, false);
        o.step(OptionKey::MinTimeStep, false);
        assert_eq!(o.min_time_step, 50);
        assert_eq!(o.fps_limit_label(), "20 fps");
        o.min_time_step = 0;
        o.step(OptionKey::MinTimeStep, true);
        assert_eq!(o.min_time_step, 0);
        assert_eq!(o.fps_limit_label(), "none");
        o.step(OptionKey::RenderDistance, false);
        assert_eq!(o.render_distance, 45);
        o.render_distance = 27;
        o.step(OptionKey::RenderDistance, false);
        assert_eq!(o.render_distance, 25);
        o.step(OptionKey::CameraSpeed, true);
        assert_eq!(o.camera_speed, 55);
        o.step(OptionKey::InvertY, true);
        assert_eq!(o.invert_y, 1);
        o.step(OptionKey::Fullscreen, false);
        assert_eq!(o.fullscreen, 1);
        assert_eq!(o.frame_sleep_ms(3), 0);
        o.min_time_step = 9;
        assert_eq!(o.frame_sleep_ms(3), 6);
        assert_eq!(o.frame_sleep_ms(12), 0);
    }
}
