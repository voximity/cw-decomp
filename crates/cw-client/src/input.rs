//! Input as `cube::GameController` sees it: the DirectInput key and button bytes WinMain copies
//! into the controller every frame, the Windows virtual-key state `cube::Controller` keeps, and
//! the hard-coded bindings of `GameController::onKeyDown` (`Cube.exe 0x0047e1b0`).
//!
//! Tier C for the devices (a winit/gilrs layer fills [`InputState`]), Tier B for what the
//! bytes mean: the game logic in [`crate::player`] and [`crate::interact`] reads only
//! [`ControllerBytes`], the image of `GameController+0x4..+0x18` and `+0x124..+0x130`, so it
//! can be driven without a window.
//!
//! Range map (Cube.exe):
//! - `WinMain` 0x004c91da..0x004c93dd: keyboard `GetDeviceState(256)` and mouse
//!   `GetDeviceState(16)` copied into the controller bytes ([`ControllerBytes::from_input`]);
//!   0x004c93e0..0x004c942a: the four stick axes at `+0x124..+0x130` zeroed (nothing in the
//!   binary writes them otherwise: see [`ControllerBytes::left_stick`]).
//! - `GameController::onKeyDown` 0x0047e1b0 (gates at 0x0047e1e0..0x0047e295, chat at
//!   0x0047e29b..0x0047e717, the VK table at 0x0047e753 with the byte index 0x0047e95c and the
//!   targets 0x0047e92c): [`on_key_down`].
//! - `GameController::onKeyUp` 0x0047e9d0: [`on_key_up`].
//! - `Controller::onMouseDown/Up` 0x0043b580/0x0043b5a0 (button state at `+0x119`).

#![allow(dead_code)]

/// DirectInput scan codes (`DIK_*`) the client reads.
pub mod dik {
    pub const KEY_1: u8 = 0x02;
    pub const KEY_2: u8 = 0x03;
    pub const KEY_3: u8 = 0x04;
    pub const KEY_4: u8 = 0x05;
    pub const Q: u8 = 0x10;
    pub const W: u8 = 0x11;
    pub const E: u8 = 0x12;
    pub const R: u8 = 0x13;
    pub const T: u8 = 0x14;
    pub const LCONTROL: u8 = 0x1d;
    pub const A: u8 = 0x1e;
    pub const S: u8 = 0x1f;
    pub const D: u8 = 0x20;
    pub const LSHIFT: u8 = 0x2a;
    pub const SPACE: u8 = 0x39;
}

/// Windows virtual-key codes (`VK_*`) the client binds.
pub mod vk {
    pub const TAB: u8 = 0x09;
    pub const RETURN: u8 = 0x0d;
    pub const ESCAPE: u8 = 0x1b;
    pub const B: u8 = 0x42;
    pub const C: u8 = 0x43;
    pub const F: u8 = 0x46;
    pub const G: u8 = 0x47;
    pub const I: u8 = 0x49;
    pub const M: u8 = 0x4d;
    pub const O: u8 = 0x4f;
    pub const V: u8 = 0x56;
    pub const X: u8 = 0x58;
    pub const F1: u8 = 0x70;
}

/// The mouse buttons in `DIMOUSESTATE::rgbButtons` order (0 left, 1 right, 2 middle), which is
/// also the button index WndProc passes to `onMouseDown`/`onMouseUp`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left = 0,
    Right = 1,
    Middle = 2,
}

/// What a device layer reports for one frame: the held DirectInput keys, the three buttons, the
/// raw mouse motion (`DIMOUSESTATE::lX/lY`, or the absolute client position while the cursor is
/// free), the wheel notches and the two sticks. Filled by the winit/gilrs layer (Tier C).
#[derive(Debug, Clone, PartialEq)]
pub struct InputState {
    /// Held keys by DirectInput scan code (the 256-byte `GetDeviceState` buffer, as booleans).
    pub keys: [bool; 256],
    /// Held mouse buttons, left, right, middle.
    pub buttons: [bool; 3],
    /// `DIMOUSESTATE::lX`, `lY` this frame (relative), or the cursor position when free.
    pub mouse: [i32; 2],
    /// The stick axes the controller keeps at `+0x124/+0x128` (left) and `+0x12c/+0x130`
    /// (right), in the XInput range -32768..32767. The original never fills them (WinMain zeroes
    /// them every frame); a gamepad layer may.
    pub left_stick: [i32; 2],
    pub right_stick: [i32; 2],
}

impl Default for InputState {
    fn default() -> Self {
        InputState { keys: [false; 256], buttons: [false; 3], mouse: [0; 2], left_stick: [0; 2], right_stick: [0; 2] }
    }
}

impl InputState {
    pub fn press(&mut self, dik: u8) {
        self.keys[usize::from(dik)] = true;
    }

    pub fn release(&mut self, dik: u8) {
        self.keys[usize::from(dik)] = false;
    }
}

/// One entry of the DirectInput table WinMain copies each frame: the key (or button) and the
/// controller byte it lands in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DikBinding {
    pub source: DikSource,
    /// Offset in `GameController` (`+0x4..+0x17`).
    pub offset: usize,
    /// What the game does with the byte (the consumers are cited in [`ControllerBytes`]).
    pub meaning: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DikSource {
    Key(u8),
    Button(MouseButton),
}

/// `WinMain` 0x004c91ef..0x004c93dd, in the original's store order: keyboard bytes first (the
/// buffer at `ebp-0x104`), then the mouse (`ebp-0x124`), with LShift written twice.
pub const DIK_BINDINGS: [DikBinding; 19] = [
    DikBinding { source: DikSource::Key(dik::W), offset: 0xb, meaning: "forward (walk, swim, climb up, roll)" },
    DikBinding { source: DikSource::Key(dik::S), offset: 0xc, meaning: "back (walk, swim, climb down, roll)" },
    DikBinding { source: DikSource::Key(dik::A), offset: 0xd, meaning: "left (strafe, climb, roll; menu previous)" },
    DikBinding { source: DikSource::Key(dik::D), offset: 0xe, meaning: "right (strafe, climb, roll; menu next)" },
    DikBinding { source: DikSource::Key(dik::E), offset: 0x11, meaning: "pick up the aimed item (Interact 5); use the selected quick item" },
    DikBinding { source: DikSource::Key(dik::R), offset: 0xf, meaning: "talk/use/respawn (Interact 2, 3, 7; mount; beds; quick item)" },
    DikBinding { source: DikSource::Key(dik::T), offset: 0x10, meaning: "call the pet (Interact 8)" },
    DikBinding { source: DikSource::Key(dik::SPACE), offset: 0x12, meaning: "jump, swim up, keep gliding" },
    DikBinding { source: DikSource::Key(dik::LSHIFT), offset: 0x14, meaning: "walk slowly (clears flag 0x40), face the aim point" },
    DikBinding { source: DikSource::Key(dik::LCONTROL), offset: 0x13, meaning: "entity flag 1 (climb)" },
    DikBinding { source: DikSource::Key(dik::Q), offset: 0x17, meaning: "use the selected quick item" },
    DikBinding { source: DikSource::Button(MouseButton::Left), offset: 0x4, meaning: "basic attack; drop the held item" },
    DikBinding { source: DikSource::Key(dik::LSHIFT), offset: 0x15, meaning: "(copy of +0x14; no reader found)" },
    DikBinding { source: DikSource::Button(MouseButton::Middle), offset: 0xa, meaning: "dodge roll with a direction key; drag the map" },
    DikBinding { source: DikSource::Button(MouseButton::Right), offset: 0x5, meaning: "skill bar slot 1 (weapon special / block)" },
    DikBinding { source: DikSource::Key(dik::KEY_1), offset: 0x6, meaning: "skill bar slot 2" },
    DikBinding { source: DikSource::Key(dik::KEY_2), offset: 0x7, meaning: "skill bar slot 3" },
    DikBinding { source: DikSource::Key(dik::KEY_3), offset: 0x8, meaning: "skill bar slot 4" },
    DikBinding { source: DikSource::Key(dik::KEY_4), offset: 0x9, meaning: "skill bar slot 5" },
];

/// The controller bytes `GameController+0x4..+0x18` and the axes at `+0x124..+0x130`, as
/// WinMain leaves them before `update` runs. Index with the original offsets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ControllerBytes {
    /// `+0x4..+0x18` (index = offset).
    pub bytes: [bool; 0x19],
    /// `+0x124`, `+0x128`: the left stick (read by `applyMovementInput` 0x004a7d76).
    pub left_stick: [i32; 2],
    /// `+0x12c`, `+0x130`: the right stick (turned into camera motion at 0x00497892).
    pub right_stick: [i32; 2],
}

impl ControllerBytes {
    /// `WinMain` 0x004c91da..0x004c942a for a focused window: every table entry's `setne`, the
    /// sticks as reported (the original zeroes them; with no gamepad layer they stay zero).
    /// `+0x16` and `+0x18` are never written by the original (the lock-on in
    /// `applyMovementInput` and the mode-0x69 key at 0x0049b5be read them): they stay false.
    pub fn from_input(input: &InputState) -> ControllerBytes {
        let mut c = ControllerBytes::default();
        for b in DIK_BINDINGS.iter() {
            let v = match b.source {
                DikSource::Key(k) => input.keys[usize::from(k)],
                DikSource::Button(m) => input.buttons[m as usize],
            };
            c.bytes[b.offset] = v;
        }
        c.left_stick = input.left_stick;
        c.right_stick = input.right_stick;
        c
    }

    #[inline]
    pub fn at(&self, offset: usize) -> bool {
        self.bytes[offset]
    }

    pub fn left_mouse(&self) -> bool {
        self.bytes[4]
    }
    pub fn right_mouse(&self) -> bool {
        self.bytes[5]
    }
    pub fn middle_mouse(&self) -> bool {
        self.bytes[0xa]
    }
    pub fn forward(&self) -> bool {
        self.bytes[0xb]
    }
    pub fn back(&self) -> bool {
        self.bytes[0xc]
    }
    pub fn left(&self) -> bool {
        self.bytes[0xd]
    }
    pub fn right(&self) -> bool {
        self.bytes[0xe]
    }
    pub fn key_r(&self) -> bool {
        self.bytes[0xf]
    }
    pub fn key_t(&self) -> bool {
        self.bytes[0x10]
    }
    pub fn key_e(&self) -> bool {
        self.bytes[0x11]
    }
    pub fn space(&self) -> bool {
        self.bytes[0x12]
    }
    pub fn ctrl(&self) -> bool {
        self.bytes[0x13]
    }
    pub fn shift(&self) -> bool {
        self.bytes[0x14]
    }
    pub fn lock_on(&self) -> bool {
        self.bytes[0x16]
    }
    pub fn key_q(&self) -> bool {
        self.bytes[0x17]
    }
    pub fn key_0x18(&self) -> bool {
        self.bytes[0x18]
    }
}

/// What a bound virtual key does (`onKeyDown` 0x0047e753 jump table).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    /// Tab (0x0047e7ae): toggle the quick-item menu, `+0x800a40`.
    ToggleQuickItems,
    /// Esc (0x0047e770): close the map (`+0x8006e4 = 0`) and toggle the system menu `+0x8008f2`.
    ToggleSystemMenu,
    /// B and I (0x0047e7c3): `toggleInventory` 0x00488c00.
    ToggleInventory,
    /// C (0x0047e805): `toggleCharacter` 0x00488bd0.
    ToggleCharacter,
    /// F (0x0047e811): toggle entity flag 0x200, the lamp (the world tick clears the flag
    /// unless equipment slot 10 holds an item of type 0x18; [`crate::player::toggle_lamp`]).
    ToggleLamp,
    /// G (0x0047e83c): the slot-11 item (type 0x17): sub type 0 the glider (flag 0x10), sub
    /// type 1 the boat (mode 0x6b in water); [`crate::player::use_special_item`].
    UseSpecialItem,
    /// M (0x0047e7db): toggle the map, `+0x8006e4`.
    ToggleMap,
    /// O (0x0047e905): `0x00488d00`, the panel at `+0x800910`.
    TogglePanelO,
    /// V (0x0047e7f0): toggle `+0x8007b4` (its readers are not identified).
    ToggleV,
    /// X (0x0047e7cf): `toggleAdaption` 0x00488c70.
    ToggleAdaption,
    /// F1 (0x0047e78c): toggle the help widget `+0x80089c`.
    ToggleHelp,
    /// Enter with the chat closed (0x0047e710): open the chat line.
    OpenChat,
}

/// The VK table, in key order (`0x0047e95c` indexed by `vk - 9`; every other key in 9..0x70 is
/// the default target 0x0047e90c, no action).
pub const VK_BINDINGS: [(u8, KeyAction); 12] = [
    (vk::TAB, KeyAction::ToggleQuickItems),
    (vk::ESCAPE, KeyAction::ToggleSystemMenu),
    (vk::B, KeyAction::ToggleInventory),
    (vk::C, KeyAction::ToggleCharacter),
    (vk::F, KeyAction::ToggleLamp),
    (vk::G, KeyAction::UseSpecialItem),
    (vk::I, KeyAction::ToggleInventory),
    (vk::M, KeyAction::ToggleMap),
    (vk::O, KeyAction::TogglePanelO),
    (vk::V, KeyAction::ToggleV),
    (vk::X, KeyAction::ToggleAdaption),
    (vk::F1, KeyAction::ToggleHelp),
];

/// The table lookup of 0x0047e753: `vk - 9` above 0x67 or a default entry is no action.
pub fn vk_action(vk: u8) -> Option<KeyAction> {
    VK_BINDINGS.iter().find(|(k, _)| *k == vk).map(|(_, a)| *a)
}

/// The UI state `onKeyDown` consults before the table (widgets belong to the UI port; the
/// controller fills this each frame).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KeyGates {
    /// Any of the five widgets at `+0x800880`, `+0x800890`, `+0x800888`, `+0x800894`,
    /// `+0x80088c` visible (0x0047e1ea..0x0047e270): every key is swallowed.
    pub blocking_panel_open: bool,
    /// `plasma::Engine::hasTextFocus` 0x006531e0 (0x0047e276).
    pub text_focus: bool,
    /// The chat line `+0x800a14` active (`+0x180`, 0x0047e289).
    pub chat_active: bool,
}

/// What `onKeyDown` does with one key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyDown {
    /// Swallowed by a panel or the text focus.
    Ignored,
    /// The chat line is active: Enter submits it (0x0047e2a6, the `/name`, `/connect`,
    /// `/disconnect`, `/namepet` commands), any other key goes to the line (0x0047e6fe).
    Chat { submit: bool },
    /// The key reached `Controller::onKeyDown` (0x0047e71f, `key[vk] = 1`) and the table.
    Key(Option<KeyAction>),
}

/// `GameController::onKeyDown` 0x0047e1b0 without the side effects that belong to the UI: the
/// gates in order, Enter opening the chat (0x0047e70b, before the VK state is set), then the
/// VK state and the table.
pub fn on_key_down(state: &mut VkState, gates: KeyGates, vk: u8) -> KeyDown {
    if gates.blocking_panel_open || gates.text_focus {
        return KeyDown::Ignored;
    }
    if gates.chat_active {
        return KeyDown::Chat { submit: vk == vk::RETURN };
    }
    if vk == vk::RETURN {
        return KeyDown::Key(Some(KeyAction::OpenChat));
    }
    state.keys[usize::from(vk)] = true;
    KeyDown::Key(vk_action(vk))
}

/// `GameController::onKeyUp` 0x0047e9d0: `Controller::onKeyUp` unless a text field has focus.
pub fn on_key_up(state: &mut VkState, text_focus: bool, vk: u8) {
    if !text_focus {
        state.keys[usize::from(vk)] = false;
    }
}

/// `cube::Controller`'s own state: the VK key array at `+0x19` and the three buttons at
/// `+0x119` (`Controller::Controller` 0x0043b4c0 zeroes both).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VkState {
    pub keys: [bool; 256],
    pub buttons: [bool; 3],
}

impl Default for VkState {
    fn default() -> Self {
        VkState { keys: [false; 256], buttons: [false; 3] }
    }
}

impl VkState {
    /// `Controller::onMouseDown` 0x0043b580.
    pub fn mouse_down(&mut self, b: MouseButton) {
        self.buttons[b as usize] = true;
    }

    /// `Controller::onMouseUp` 0x0043b5a0.
    pub fn mouse_up(&mut self, b: MouseButton) {
        self.buttons[b as usize] = false;
    }
}

/// The per-key edge latches `update` keeps in globals: each is the key's value at the end of
/// the previous frame, so "pressed" means `now && !latch`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EdgeLatches {
    /// `0x0076b138` R, `0x0076b139` E, `0x0076b13a` T (stored at 0x0049b593..0x0049b5a6).
    pub r: bool,
    pub e: bool,
    pub t: bool,
    /// `0x0076b13b` left mouse (0x004966c9).
    pub left_mouse: bool,
    /// `0x0076b100` Q (0x0049266a region).
    pub q: bool,
    /// `0x0076b14c` A, `0x0076b14d` D in the build menu (0x00498034).
    pub build_a: bool,
    pub build_d: bool,
    /// `0x0076b14e` the `+0x18` byte (0x0049b5fb).
    pub key_0x18: bool,
    /// `0x0076b160` the lock-on byte `+0x16` (`applyMovementInput` 0x004a8cae).
    pub lock_on: bool,
    /// `0x0076b161` Space (`applyMovementInput` 0x004a8dd6; written, never read).
    pub space: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dik_table_matches_winmain() {
        // 0x004c91f2..0x004c93dd: (buffer offset below ebp-0x104 -> DIK, controller byte).
        let expect: [(u8, usize); 19] = [
            (0x11, 0xb),
            (0x1f, 0xc),
            (0x1e, 0xd),
            (0x20, 0xe),
            (0x12, 0x11),
            (0x13, 0xf),
            (0x14, 0x10),
            (0x39, 0x12),
            (0x2a, 0x14),
            (0x1d, 0x13),
            (0x10, 0x17),
            (0x2a, 0x15),
            (0x02, 0x6),
            (0x03, 0x7),
            (0x04, 0x8),
            (0x05, 0x9),
            (0, 0),
            (0, 0),
            (0, 0),
        ];
        let keys: Vec<(u8, usize)> = DIK_BINDINGS
            .iter()
            .filter_map(|b| match b.source {
                DikSource::Key(k) => Some((k, b.offset)),
                DikSource::Button(_) => None,
            })
            .collect();
        for (k, o) in expect.iter().filter(|(k, _)| *k != 0) {
            assert!(keys.contains(&(*k, *o)), "missing DIK {k:#x} -> +{o:#x}");
        }
        assert_eq!(keys.len(), 16);
        let buttons: Vec<(MouseButton, usize)> = DIK_BINDINGS
            .iter()
            .filter_map(|b| match b.source {
                DikSource::Button(m) => Some((m, b.offset)),
                DikSource::Key(_) => None,
            })
            .collect();
        assert_eq!(buttons, vec![(MouseButton::Left, 4), (MouseButton::Middle, 0xa), (MouseButton::Right, 5)]);
    }

    #[test]
    fn controller_bytes_from_input() {
        let mut i = InputState::default();
        i.press(dik::W);
        i.press(dik::LSHIFT);
        i.buttons[MouseButton::Right as usize] = true;
        i.press(dik::KEY_4);
        let c = ControllerBytes::from_input(&i);
        assert!(c.forward() && !c.back());
        assert!(c.shift() && c.at(0x15));
        assert!(c.right_mouse() && !c.left_mouse() && !c.middle_mouse());
        assert!(c.at(9) && !c.at(6));
        // Never written by the original.
        assert!(!c.lock_on() && !c.key_0x18());
    }

    #[test]
    fn vk_table_matches_jump_table() {
        // Byte table 0x0047e95c (index vk - 9) into the targets at 0x0047e92c.
        let expect: [(u8, KeyAction); 12] = [
            (0x09, KeyAction::ToggleQuickItems),
            (0x1b, KeyAction::ToggleSystemMenu),
            (0x42, KeyAction::ToggleInventory),
            (0x43, KeyAction::ToggleCharacter),
            (0x46, KeyAction::ToggleLamp),
            (0x47, KeyAction::UseSpecialItem),
            (0x49, KeyAction::ToggleInventory),
            (0x4d, KeyAction::ToggleMap),
            (0x4f, KeyAction::TogglePanelO),
            (0x56, KeyAction::ToggleV),
            (0x58, KeyAction::ToggleAdaption),
            (0x70, KeyAction::ToggleHelp),
        ];
        for (k, a) in expect {
            assert_eq!(vk_action(k), Some(a), "vk {k:#x}");
        }
        for k in 0u8..=0xff {
            if !expect.iter().any(|(e, _)| *e == k) {
                assert_eq!(vk_action(k), None, "vk {k:#x}");
            }
        }
    }

    #[test]
    fn key_down_gates() {
        let mut s = VkState::default();
        let g = KeyGates { blocking_panel_open: true, ..KeyGates::default() };
        assert_eq!(on_key_down(&mut s, g, vk::F), KeyDown::Ignored);
        assert!(!s.keys[usize::from(vk::F)]);
        let g = KeyGates { chat_active: true, ..KeyGates::default() };
        assert_eq!(on_key_down(&mut s, g, vk::RETURN), KeyDown::Chat { submit: true });
        let g = KeyGates::default();
        assert_eq!(on_key_down(&mut s, g, vk::RETURN), KeyDown::Key(Some(KeyAction::OpenChat)));
        assert!(!s.keys[usize::from(vk::RETURN)]);
        assert_eq!(on_key_down(&mut s, g, vk::G), KeyDown::Key(Some(KeyAction::UseSpecialItem)));
        assert!(s.keys[usize::from(vk::G)]);
        on_key_up(&mut s, false, vk::G);
        assert!(!s.keys[usize::from(vk::G)]);
    }
}
