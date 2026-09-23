//! `cube::SpeechWidget` (vtable 0x00703e34, ctor 0x004e5c90, 0x1e0 bytes): the typewriter
//! speech bubbles and the NPC dialog (+0x800938), update body 0x004e5f90.
//!
//! | Field | Offset |
//! |---|---|
//! | pages (`vector<list<run>>`, 8-byte entries) | +0x160 |
//! | elapsed reveal time (ms) | +0x16c |
//! | page index | +0x170 |
//! | previous reveal count | +0x174 |
//! | answer options (`vector<wstring>`, 0x18 each) | +0x178..+0x17c |
//! | hovered option | +0x184 |
//! | font name `resource1.dat` | +0x1a8 |
//! | option font size 14 / outline 3 | +0x1c0 / +0x1c4 |
//! | layout cursor and reveal count, reset each frame | +0x1c8..+0x1d0 |
//! | milliseconds per character, 40 | +0x1d4 |
//! | GameController* | +0x1d8 |
//!
//! Each frame the runs of the current page are laid out by 0x004e65a0, which draws the
//! revealed characters (the renderer's business) and counts them into +0x1d0
//! ([`SpeechWidget::revealed`]). Every third newly revealed
//! character plays sound 0x32 at the local player with pitch `rand() * 0.5 / 32767 + 1` (one
//! CRT `rand()` draw on the main thread: the same stream the client's world tick uses).
//! Once the whole page has had its time (`(chars + runs) * 40 <= elapsed`), the answer
//! options are drawn in a row at `y = (int)(height - 75) + origin.y`, x from 15 in steps of
//! `20 + width`, the hovered one in `(0, 1, 1, 1)`.

use cw_math::rand::MsvcRand;

use super::UiAction;

/// The widget's state.
#[derive(Clone, Debug, PartialEq)]
pub struct SpeechWidget {
    /// +0x160: per page, the character count of each text run.
    pub pages: Vec<Vec<usize>>,
    /// +0x16c.
    pub elapsed_ms: i32,
    /// +0x170.
    pub page: i32,
    /// +0x174.
    pub prev_revealed: i32,
    /// +0x178.
    pub options: Vec<String>,
    /// +0x184.
    pub hovered: i32,
    /// +0x1d4.
    pub ms_per_char: i32,
    /// +0x188 (cleared with the node hidden, 0x00496886 / 0x004967e3; meaning not traced).
    pub f188: i32,
    /// The words of each page (`+0x160` holds `std::list<std::pair<wstring, float4>>` per
    /// page; [`SpeechWidget::pages`] keeps their UTF-16 lengths).
    pub texts: Vec<Vec<super::textdb::Word>>,
    /// `+0x188` of a speech bubble (`GC+0x80092c[i]`): the creature it follows (a
    /// `cube::Creature*` in the original, its id here; 0 = free). Written by 0x004882e0 /
    /// 0x00488030, cleared by the page step (0x004967e3).
    pub creature: i64,
    /// `+0x190..+0x1a8`: the bubble's anchor (fixed point), eased towards the creature by
    /// `update` 0x00496f37..0x00497078.
    pub pos: [i64; 3],
}

/// The option font size `+0x1c0` and outline `+0x1c4` (ctor 0x004e5c90: 14, 3).
pub const OPTION_SIZE: f32 = 14.0;
/// See [`OPTION_SIZE`].
pub const OPTION_OUTLINE: f32 = 3.0;

/// The widget geometry the ctor 0x004e5c90 gives itself through `0x00627c00(widget, pos
/// (0, 0), size (250, 200), inner pos (10, 10), inner size (230, 180), identity)`:
/// `+0x48` inner bind pos, `+0x50` inner bind size, `+0x58`/`+0x68` bind and frame pos,
/// `+0x60`/`+0x70` bind and frame size. The bubbles wrap against its width (0x004e67a0).
pub fn widget_source() -> cw_ui::widget::WidgetSource {
    use glam::Vec2;
    cw_ui::widget::WidgetSource {
        inner_bind_pos: Vec2::new(10.0, 10.0),
        inner_bind_size: Vec2::new(230.0, 180.0),
        bind_pos: Vec2::ZERO,
        bind_size: Vec2::new(250.0, 200.0),
        frame_pos: Vec2::ZERO,
        frame_size: Vec2::new(250.0, 200.0),
        ..Default::default()
    }
}

/// The rest of `0x00627c00` on a widget made from [`widget_source`]: `+0x78 = bind size −
/// inner bind size` (20, 20).
pub fn init_widget(gui: &mut cw_ui::widget::Gui, w: cw_ui::widget::WidgetId) {
    let wd = &mut gui.widgets[w];
    wd.min_frame_size = wd.bind_size - wd.inner_bind_size;
}

impl Default for SpeechWidget {
    /// ctor 0x004e5c90: zeros, 40 ms per character.
    fn default() -> Self {
        SpeechWidget {
            pages: Vec::new(),
            elapsed_ms: 0,
            page: 0,
            prev_revealed: 0,
            options: Vec::new(),
            hovered: 0,
            ms_per_char: 0x28,
            f188: 0,
            texts: Vec::new(),
            creature: 0,
            pos: [0; 3],
        }
    }
}

impl SpeechWidget {
    /// `0x004681e0(lines)` / `0x00486780(line)`: the rendered lines become the pages (each
    /// word a run; its length the `std::wstring` size in UTF-16 units).
    pub fn set_lines(&mut self, lines: &super::textdb::Lines) {
        self.pages = lines.iter().map(|l| l.iter().map(|w| w.text.encode_utf16().count()).collect()).collect();
        self.texts = lines.clone();
    }

    /// 0x004e6530: the current page has had its full reveal time (`page_time <= elapsed`).
    pub fn page_done(&self) -> bool {
        self.page_time() <= self.elapsed_ms
    }

    fn page_valid(&self) -> bool {
        -1 < self.page && (self.page as usize) < self.pages.len()
    }

    /// The character count the layout pass 0x004e65a0 leaves in `+0x1d0` (reset to 0 each
    /// frame): with `limit = elapsed / ms_per_char` (signed), the runs of the current page are
    /// walked in order; an empty run is skipped; a run starting while `count <= limit` adds
    /// `min(len, limit - count) + 1` (the `+1` is the run's space); the first run starting
    /// past the limit stops the walk. (The original counts only when the font
    /// `resource1.dat` exists, 0x00639800; the port assumes it does.) 0 without a valid page.
    pub fn revealed(&self) -> i32 {
        if !self.page_valid() || self.ms_per_char == 0 {
            return 0;
        }
        let limit = self.elapsed_ms / self.ms_per_char;
        let mut count: i32 = 0;
        for &len in &self.pages[self.page as usize] {
            let len = len as i32;
            if len == 0 {
                continue;
            }
            if count > limit {
                break;
            }
            let n = if count + len > limit { limit - count } else { len };
            count += n + 1;
        }
        count
    }

    /// 0x004e6550: the page's total reveal time, `sum(1 + run length) * ms_per_char` (0
    /// without a valid page).
    pub fn page_time(&self) -> i32 {
        if !self.page_valid() {
            return 0;
        }
        self.pages[self.page as usize].iter().map(|&n| 1 + n as i32).sum::<i32>() * self.ms_per_char
    }

    /// `update` 0x004908bf: `+0x16c += dt` (the dialog widget +0x800938 only; the bubbles'
    /// clocks have no writer besides the page step below).
    pub fn advance(&mut self, dt: i32) {
        self.elapsed_ms = self.elapsed_ms.wrapping_add(dt);
    }

    /// The page step of `update` 0x00496700..0x004967ed (bubbles) / 0x00496805..0x00496886
    /// (dialog): without answer options (0x00477240, the vector is empty), once the page has
    /// been shown 3 s past its reveal time (`elapsed > page_time + 3000`) the next page
    /// starts (`+0x170 += 1`, `+0x16c = 0`, `+0x174 = 0`). Returns `true` when the page index
    /// is past the last page: the caller hides the widget's node and clears `+0x188`.
    pub fn page_step(&mut self) -> bool {
        if self.options.is_empty() && self.elapsed_ms > self.page_time() + 0xbb8 {
            self.page += 1;
            self.elapsed_ms = 0;
            self.prev_revealed = 0;
        }
        if self.page >= self.pages.len() as i32 {
            self.f188 = 0;
            self.creature = 0;
            return true;
        }
        false
    }

    /// The answer options of 0x004e5f90 once the page has had its time (`show`): drawn in a
    /// row at `y = (int)(height − 75) + pivot.y`, `x = 15 + pivot.x` advancing by
    /// `20 + (int)(x1 − x0)` of each option's extent (0x0065e720 at size `+0x1c0`, no
    /// stroke). Returns `(text, x, y, hovered)`; `pivot` is the widget node's
    /// Transformation pivot (`node+0x38 → +0x19c[+0x170]`).
    pub fn option_layout(&self, height: f32, pivot: [f32; 2], measure: &dyn Fn(&str) -> f32) -> Vec<(String, f32, f32, bool)> {
        let y = ((height - 75.0f32) as i32) as f32 + pivot[1];
        let mut x = 0xf;
        let mut out = Vec::new();
        for (i, o) in self.options.iter().enumerate() {
            out.push((o.clone(), x as f32 + pivot[0], y, i as i32 == self.hovered));
            x += 0x14 + measure(o) as i32;
        }
        out
    }

    /// The update body 0x004e5f90 after the layout pass: `revealed` is what 0x004e65a0
    /// counted into +0x1d0 this frame. Returns the typing sound when it plays and whether the
    /// options are shown.
    pub fn update(&mut self, revealed: i32, rng: &mut MsvcRand) -> (Option<UiAction>, bool) {
        if !self.page_valid() {
            return (None, false);
        }
        let mut sound = None;
        if self.prev_revealed / 3 != revealed / 3 {
            let r = rng.rand();
            let pitch = (r as f32 * 0.5f32) / 32767.0f32 + 1.0f32;
            sound = Some(UiAction::PlaySound { id: 0x32, volume: 1.0, pitch });
        }
        self.prev_revealed = revealed;
        // Total time of the page: `sum(1 + run length) * ms_per_char`.
        let total: i32 = self.pages[self.page as usize].iter().map(|&n| 1 + n as i32).sum::<i32>() * self.ms_per_char;
        let show = total <= self.elapsed_ms && !self.options.is_empty();
        (sound, show)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revealed_counts() {
        let mut s = SpeechWidget { pages: vec![vec![5, 0, 3]], ..Default::default() };
        assert_eq!(s.revealed(), 1);
        s.elapsed_ms = 80;
        assert_eq!(s.revealed(), 3);
        s.elapsed_ms = 400;
        assert_eq!(s.revealed(), 10);
        s.page = 1;
        assert_eq!(s.revealed(), 0);
    }

    #[test]
    fn pages_advance_and_end() {
        let mut s = SpeechWidget { pages: vec![vec![2]], ..Default::default() };
        // Page time (1 + 2) * 40 = 120; the page moves on past 120 + 3000.
        s.advance(3120);
        assert!(!s.page_step());
        s.advance(1);
        assert!(s.page_step());
        assert_eq!((s.page, s.elapsed_ms, s.prev_revealed), (1, 0, 0));
        // With options the page stays.
        let mut s = SpeechWidget { pages: vec![vec![2]], options: vec!["Yes".into(), "No".into()], hovered: 1, ..Default::default() };
        s.advance(10_000);
        assert!(!s.page_step());
        let o = s.option_layout(400.0, [0.0, 0.0], &|t: &str| 10.0 * t.len() as f32);
        assert_eq!(o, vec![("Yes".to_string(), 15.0, 325.0, false), ("No".to_string(), 65.0, 325.0, true)]);
        // No pages at all: hidden.
        assert!(SpeechWidget::default().page_step());
    }

    #[test]
    fn typing_sound_every_three_characters() {
        let mut s = SpeechWidget { pages: vec![vec![5, 3]], options: vec!["Bye".into()], ..Default::default() };
        let mut r = MsvcRand::new(1);
        let (a, show) = s.update(2, &mut r);
        assert!(a.is_none() && !show);
        let (a, _) = s.update(3, &mut r);
        // The first MSVC rand() of seed 1 is 41: pitch = 41 * 0.5 / 32767 + 1.
        assert_eq!(a, Some(UiAction::PlaySound { id: 0x32, volume: 1.0, pitch: 41.0 * 0.5 / 32767.0 + 1.0 }));
        s.elapsed_ms = (6 + 4) * 40;
        let (_, show) = s.update(3, &mut r);
        assert!(show);
    }
}
