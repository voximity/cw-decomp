//! `cube::ChatWidget` (vtable 0x006fd78c, ctor 0x004393b0, 0x190 bytes) and the chat input
//! handling of `GameController::onKeyDown` 0x0047e1b0 / `onChar` 0x00482a00.
//!
//! | Field | Offset | Rust |
//! |---|---|---|
//! | line list (list of token lists) | +0x160 / count +0x164 | [`ChatWidget::lines`] |
//! | input `std::wstring` (length +0x178) | +0x168 | [`ChatWidget::input`] |
//! | input active | +0x180 | [`ChatWidget::input_active`] |
//! | font `resource1.dat` | +0x184 | (the renderer's) |
//! | caret | +0x188 | [`ChatWidget::caret`] |
//! | signed selection length | +0x18c | [`ChatWidget::selection`] |
//! | Widget dirty | +0x134 | [`ChatWidget::dirty`] |
//!
//! The ctor also sets widget flag 0x40 (`+0x128 |= 0x40`).

use std::collections::VecDeque;

use glam::Vec2;

use super::flow::narrow;
use super::{GameView, UiAction};

/// One word or separator of a printed line with its RGB colour.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    /// The text (a word, `" "`, or a line break).
    pub text: String,
    /// RGB bytes (the alpha byte the original stores is never read).
    pub color: [u8; 3],
}

/// A text item the update body 0x00439730 draws: black shadow pass then the fill.
#[derive(Clone, Debug, PartialEq)]
pub struct ChatDraw {
    /// The text.
    pub text: String,
    /// Baseline position.
    pub pos: Vec2,
    /// RGBA fill.
    pub color: [f32; 4],
}

/// The widget's state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChatWidget {
    /// +0x160: at most 10 lines are kept (the update body pops the oldest).
    pub lines: VecDeque<Vec<Token>>,
    /// +0x168.
    pub input: Vec<u16>,
    /// +0x180.
    pub input_active: bool,
    /// +0x188.
    pub caret: i32,
    /// +0x18c.
    pub selection: i32,
    /// +0x134.
    pub dirty: bool,
}

/// Text size and spacing of every chat draw and measure (10, 2).
pub const FONT_SIZE: f32 = 10.0;

/// Virtual keys the edit handler reads.
mod vk {
    pub const BACK: u16 = 0x08;
    pub const RETURN: u16 = 0x0d;
    pub const END: u16 = 0x23;
    pub const HOME: u16 = 0x24;
    pub const LEFT: u16 = 0x25;
    pub const RIGHT: u16 = 0x27;
    pub const DELETE: u16 = 0x2e;
}

impl ChatWidget {
    fn len(&self) -> i32 {
        self.input.len() as i32
    }

    /// 0x0043a4a0: clamps the caret to `0..=len` and the selection so `caret + sel` stays in
    /// range (the last test moves the caret, not the selection).
    pub fn clamp(&mut self) {
        if self.caret < 0 {
            self.caret = 0;
        }
        let len = self.len();
        if len < self.caret {
            self.caret = len;
        }
        if self.selection + self.caret < 0 {
            self.selection = -self.caret;
        }
        if len < self.selection + self.caret {
            self.caret = len - self.selection;
        }
    }

    /// 0x004396d0: deletes the selection, or the character at the caret when there is none.
    pub fn delete_selection(&mut self) {
        self.clamp();
        let mut start = self.caret;
        if start <= self.len() {
            let mut n = self.selection;
            if n == 0 {
                n = 1;
            } else if n < 0 {
                start += n;
                n = -n;
            }
            // `wstring::erase(pos, n)` clamps `n` to what remains.
            let s = start as usize;
            let e = (s + n as usize).min(self.input.len());
            self.input.drain(s..e);
            if self.selection < 0 {
                self.caret += self.selection;
            }
            self.selection = 0;
        }
    }

    /// `ChatWidget::onChar` 0x0043a010 (via `GameController::onChar` 0x00482a00 while the
    /// input is active and no widget has focus).
    pub fn on_char(&mut self, c: u16) {
        self.clamp();
        if c >= 0x20 {
            if self.selection != 0 {
                self.delete_selection();
            }
            self.clamp();
            self.input.insert(self.caret as usize, c);
            self.caret += 1;
        }
    }

    /// `ChatWidget` key handler 0x0043a0d0 (`shift` = `isKeyDown(VK_SHIFT)` 0x0043a3f0).
    pub fn on_key(&mut self, key: u16, shift: bool) {
        self.clamp();
        self.dirty = true;
        match key {
            vk::RETURN => {}
            vk::LEFT => {
                if 0 < self.caret {
                    self.caret -= 1;
                    if shift {
                        self.selection += 1;
                    } else {
                        self.selection = 0;
                    }
                }
            }
            vk::RIGHT => {
                if self.caret < self.len() {
                    self.caret += 1;
                    if shift {
                        self.selection -= 1;
                    } else {
                        self.selection = 0;
                    }
                }
            }
            vk::DELETE => self.delete_selection(),
            vk::BACK => {
                if self.selection != 0 {
                    self.delete_selection();
                } else if 0 < self.caret {
                    // 0x00439fc0: erase the character before the caret.
                    self.input.remove(self.caret as usize - 1);
                    self.caret -= 1;
                }
            }
            vk::HOME => {
                if shift {
                    self.selection += self.caret;
                } else {
                    self.selection = 0;
                }
                self.caret = 0;
            }
            vk::END => {
                if shift {
                    self.selection += self.caret - self.len();
                } else {
                    self.selection = 0;
                }
                self.caret = self.len();
            }
            _ => {}
        }
    }

    /// The Enter branch of `onKeyDown` 0x0047e1b0 with the input active: a non-empty input is
    /// read word by word (`wistringstream >> wstring`, 0x00451210) and the commands `/name`,
    /// `/connect`, `/disconnect` and `/namepet` run; anything else is chat. The input is then
    /// cleared and closed.
    pub fn enter(&mut self, game: &GameView) -> Vec<UiAction> {
        let mut out = Vec::new();
        self.dirty = true;
        if !self.input.is_empty() {
            let text = String::from_utf16_lossy(&self.input);
            let mut words = text.split_whitespace();
            let first = words.next().unwrap_or("");
            match first {
                "/name" => {
                    let w = words.next().unwrap_or("");
                    if !w.is_empty() {
                        out.push(UiAction::SetName { name: narrow(w) });
                    }
                }
                "/connect" => {
                    let w = words.next().unwrap_or("");
                    out.push(UiAction::Connect { address: narrow(w) });
                }
                "/disconnect" => out.push(UiAction::Disconnect),
                "/namepet" => {
                    let w = words.next().unwrap_or("");
                    if w.encode_utf16().count() < 0x10 {
                        // Only when the pet slot (`creature+0x1020`, equipment k = 11) holds a
                        // pet (item type 0x13).
                        if game.equipment.get(11).is_some_and(|i| i[0] == 0x13) {
                            out.push(UiAction::SetPetName { name: narrow(w) });
                        }
                    } else {
                        out.push(UiAction::Print { text: "Pet name too long".into(), color: [1.0, 0.2, 0.2, 1.0] });
                    }
                }
                _ => out.push(UiAction::SendChat { text, local_echo: !game.socket_open }),
            }
            self.input.clear();
            self.caret = 0;
            self.selection = 0;
        }
        self.input_active = false;
        out
    }

    /// `ChatWidget::print` 0x0043a500: splits `text` into words and separators (`' '`,
    /// `'\n'`, `'\r'` each become their own token) and appends them to the last line,
    /// wrapping at `(int)(width - 20)`. `measure` gives a token's extent at size 10, spacing 2
    /// (0x0065e720: `x1 - x0`).
    ///
    /// Wrapping: the line's current width is the sum of its tokens (a space counts 5). A line
    /// break token starts a new line. A token that would reach the limit starts a new line
    /// (a space is then dropped) — except the first overflow after appending to a line that
    /// ends in a word, which is kept on the line (the `var_69` flag of the original).
    pub fn print(&mut self, text: &str, color: [u8; 3], width: f32, measure: &dyn Fn(&str) -> f32) {
        let u: Vec<u16> = text.encode_utf16().collect();
        let mut tokens: Vec<String> = Vec::new();
        let mut start = 0usize;
        let len = u.len();
        for i in 0..=len {
            let sep = i == len || matches!(u[i], 0x20 | 0x0a | 0x0d);
            if sep {
                if i > start {
                    tokens.push(String::from_utf16_lossy(&u[start..i]));
                }
                if i != len {
                    tokens.push(String::from_utf16_lossy(&u[i..i + 1]));
                }
                start = i + 1;
            }
        }
        let limit = (width - 20.0) as i32;
        if self.lines.is_empty() {
            self.lines.push_back(Vec::new());
        }
        let tw = |t: &str| -> i32 { if t == " " { 5 } else { measure(t) as i32 } };
        let mut cur: f32 = 0.0;
        for t in self.lines.back().unwrap() {
            cur = if t.text == " " { cur + 5.0 } else { measure(&t.text) + cur };
        }
        let mut cont = self.lines.back().unwrap().last().is_some_and(|t| t.text != " ");
        for t in tokens {
            if t == "\n" || t == "\r" {
                self.lines.push_back(Vec::new());
                cur = 0.0;
                continue;
            }
            let w = tw(&t);
            cur += w as f32;
            let mut append = true;
            if !(cur < limit as f32) {
                if !cont {
                    self.lines.push_back(Vec::new());
                    cur = w as f32;
                    append = t != " ";
                } else {
                    cont = false;
                }
            }
            if append {
                self.lines.back_mut().unwrap().push(Token { text: t, color });
            }
        }
        self.dirty = true;
    }

    /// One entry of the received chat `GameController+0x1000e58`, as `update` prints it
    /// (0x0048cc0f..0x0048cd99, one entry per frame, then `pop_front` 0x00486030): when the
    /// sender id is `>= 0` and `World::findEntity` 0x0042f000 finds it, its name
    /// (`creature+0x1168` widened by 0x006089c0) `+ L": "` (0x00451800, string 0x00701850) in
    /// (0, 255, 255); then the text `+ L"\n"` (0x00451850, string 0x006fd84c) in white. Each
    /// through [`ChatWidget::print`] 0x0043a500. `name` is `None` when there is no sender.
    pub fn print_received(&mut self, name: Option<&str>, text: &str, width: f32, measure: &dyn Fn(&str) -> f32) {
        if let Some(n) = name {
            self.print(&format!("{n}: "), [0, 255, 255], width, measure);
        }
        self.print(&format!("{text}\n"), [255, 255, 255], width, measure);
    }

    /// `ChatWidget::update` 0x00439730: drops the oldest lines while more than 10 remain,
    /// then lays out the lines from y = 19 in 18 px steps (x from 3; a space advances 5), the
    /// input at `(3, height - 12)` and, while the input is active and `engine_time / 500` is
    /// odd, the caret `"|"` at `(caret_x + 3, height - 12)`.
    pub fn update(&mut self, height: f32, engine_time_ms: i32, measure: &dyn Fn(&str) -> f32) -> Vec<ChatDraw> {
        while self.lines.len() > 10 {
            self.lines.pop_front();
        }
        let mut out = Vec::new();
        let mut y = 0x13;
        for line in &self.lines {
            let mut x = 3.0f32;
            for t in line {
                if t.text == " " {
                    x += 5.0;
                } else {
                    let c = [t.color[0] as f32 / 255.0, t.color[1] as f32 / 255.0, t.color[2] as f32 / 255.0, 1.0];
                    out.push(ChatDraw { text: t.text.clone(), pos: Vec2::new(x, y as f32), color: c });
                    x = measure(&t.text) + x;
                }
            }
            y += 0x12;
        }
        let input = String::from_utf16_lossy(&self.input);
        out.push(ChatDraw { text: input, pos: Vec2::new(3.0, height - 12.0), color: [1.0; 4] });
        if self.input_active && (engine_time_ms / 500) % 2 != 0 {
            self.clamp();
            let before = String::from_utf16_lossy(&self.input[..self.caret as usize]);
            let cx = measure(&before);
            out.push(ChatDraw { text: "|".into(), pos: Vec2::new(cx + 3.0, height - 12.0), color: [1.0; 4] });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(s: &str) -> f32 {
        6.0 * s.chars().count() as f32
    }

    #[test]
    fn editing() {
        let mut c = ChatWidget::default();
        for ch in "hello".encode_utf16() {
            c.on_char(ch);
        }
        c.on_key(0x24, true); // shift+Home selects everything before the caret
        assert_eq!((c.caret, c.selection), (0, 5));
        c.on_char('J' as u16);
        assert_eq!(String::from_utf16_lossy(&c.input), "J");
        c.on_key(0x08, false);
        assert!(c.input.is_empty());
        for ch in "/connect 10.0.0.1".encode_utf16() {
            c.on_char(ch);
        }
        c.input_active = true;
        let a = c.enter(&GameView::default());
        assert_eq!(a, vec![UiAction::Connect { address: "10.0.0.1".into() }]);
        assert!(!c.input_active && c.input.is_empty());
        for ch in "hi all".encode_utf16() {
            c.on_char(ch);
        }
        let a = c.enter(&GameView::default());
        assert_eq!(a, vec![UiAction::SendChat { text: "hi all".into(), local_echo: true }]);
    }

    /// One received chat entry per frame (0x0048cc0f..0x0048cd99): the sender's name and
    /// `": "` in (0, 255, 255), then the text and `"\n"` in white, so each message is its own
    /// line; no prefix without a sender.
    #[test]
    fn received_lines() {
        let mut c = ChatWidget::default();
        c.print_received(Some("Player"), "hello there", 400.0, &m);
        c.print_received(Some("Player"), "again", 400.0, &m);
        c.print_received(None, "anon", 400.0, &m);
        let text = |l: &Vec<Token>| l.iter().map(|t| t.text.as_str()).collect::<String>();
        let lines: Vec<String> = c.lines.iter().map(text).collect();
        assert_eq!(lines, ["Player: hello there", "Player: again", "anon", ""]);
        let cyan = [0, 255, 255];
        let white = [255, 255, 255];
        assert_eq!(c.lines[0][0], Token { text: "Player:".into(), color: cyan });
        assert_eq!(c.lines[0][1], Token { text: " ".into(), color: cyan });
        assert!(c.lines[0][2..].iter().all(|t| t.color == white));
        assert!(c.lines[2].iter().all(|t| t.color == white));
    }

    #[test]
    fn wrapping() {
        let mut c = ChatWidget::default();
        // limit = 80 - 20 = 60 px; words are 6 px per char.
        c.print("aaaa bbbb cccc dddd", [255, 255, 255], 80.0, &m);
        // "aaaa"(24) " "(29) "bbbb"(53) " "(58) "cccc" -> 82 >= 60: new line.
        assert_eq!(c.lines.len(), 2);
        assert_eq!(c.lines[0].iter().map(|t| t.text.as_str()).collect::<String>(), "aaaa bbbb ");
        c.print("x\ny", [255, 0, 0], 80.0, &m);
        let d = c.update(200.0, 500, &m);
        assert_eq!(d[0].pos, Vec2::new(3.0, 19.0));
        assert!(d.iter().any(|x| x.text == "y" && x.pos.y == 19.0 + 36.0));
    }
}
