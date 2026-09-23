//! The menu flow: start screen → character select → character creation → world select →
//! world creation / server connect → in game, plus the in-game panel toggles.
//!
//! Each screen is one unnamed root node the ctor creates (`GcMembers::*_root`); a screen is
//! "shown" by its Display visibility. The handlers below are the GameController member
//! functions the ctor connects to the screens' buttons; the per-frame rules are the first
//! ranges of `GameController::update` 0x00488ee0 (Ghidra lines 300..700 of the decompilation,
//! 0x00488f..0x00489ca0).
//!
//! ```text
//!  start (0x800874) --"Start Game"--> char select (0x800888) --Select(new)--> char create (0x800880)
//!        |                               |  Select(existing)                    | "Create Character"
//!        |"Options"--> options panel     v                                      v
//!        |"Exit"--> quit             world select (0x80088c) <------------------+
//!                                        |  Select(new) --> world create (0x800890) --"Create World"--> in game
//!                                        |  Select(existing) --> in game (startWorld)
//!                                        |  "Connect to server" --> server (0x800894) --"Connect"--> connect()
//!  Back: world create -> world select -> char select -> start; char create -> char select;
//!        server -> world select
//! ```

use cw_ui::widget::{NodeId, WidgetKind};

use super::members::Handler;
use super::{CharacterEntry, GameUi, GameView, UiAction, WorldEntry};

// ---------------------------------------------------------------------------------------
// Text of the labelled edits (`Node::childText` 0x00635550 / `setChildText` 0x00636a00)
// ---------------------------------------------------------------------------------------

/// The node holding an edit's text. `Node::childText(name, 1)` 0x00635550 finds the first
/// node named `edit` and reads the text of its subtree (0x00634940); the shipped `edit`
/// template (`gui.plx`) is `edit` (plain widget) > `clip` > `text` (the `plasma::Edit`,
/// whose own TextShape holds the string, `Edit+0x160`). The Edit's text node when it has
/// one, else the first node of the subtree (depth first, itself included) carrying text.
fn edit_text_node(ui: &GameUi, label: Option<NodeId>) -> Option<NodeId> {
    let label = label?;
    let e = super::members::find_node(&ui.gui, label, "edit")?;
    let mut stack = vec![e];
    let mut first_text = None;
    while let Some(n) = stack.pop() {
        if let Some(w) = ui.gui.nodes[n].widget
            && let WidgetKind::Edit(d) = &ui.gui.widgets[w].kind
        {
            return Some(d.text_node.unwrap_or(n));
        }
        if first_text.is_none() && ui.gui.nodes[n].text.is_some() {
            first_text = Some(n);
        }
        stack.extend(ui.gui.nodes[n].children.iter().rev());
    }
    Some(first_text.unwrap_or(e))
}

/// `0x00635550("edit", ...)` on a label node: the text of its `edit` child.
pub fn edit_text(ui: &GameUi, label: Option<NodeId>) -> String {
    edit_text_node(ui, label)
        .and_then(|n| ui.gui.nodes[n].text.as_ref())
        .map(|t| String::from_utf16_lossy(t))
        .unwrap_or_default()
}

/// `0x00636a00("edit", text, 1)` on a label node: every TextShape under the `edit` node
/// takes `text` ([`super::present_hud::set_named_text`]); the Edit's text node too when it
/// carries no TextShape (the port's fallback nodes).
pub fn set_edit_text(ui: &mut GameUi, label: Option<NodeId>, text: &str) {
    if let Some(l) = label {
        super::present_hud::set_named_text(&mut ui.gui, l, "edit", text);
    }
    if let Some(n) = edit_text_node(ui, label) {
        ui.gui.nodes[n].text = Some(text.encode_utf16().collect());
    }
    // The Edit's caret/selection are clamped by its next update (0x00638cf0).
}

/// The ctor's text of the loading label (0x006ffe74, node name `wait` 0x006ffe94).
pub const WAIT_TEXT: &str = "Please wait...";

/// One TextShape of the loading label (ctor 0x0045b576..0x0045b7d0, each
/// `Engine::createTextShape(L"", L"")` 0x006502e0): fill colour `+0xb4` white, pixel size
/// `+0x1bc` 12, line spacing `+0x1c8` 3, the string `+0x5c` "Please wait..." (0x00467f60;
/// the fill copies it, 0x00467f30), font `+0x1cc` `resource1.dat` (0x00468000 with
/// 0x006fcd24), `0x00487e80(1)` (flags `+0x1ec` 1, centred), `rebuild(1)` (slot 1). The
/// outline shape also takes the stroke colour `+0x10c` (0, 0, 0, 1) and radius `+0x1c0` 3.
fn wait_text_shape(stroke: bool) -> cw_ui::loader::SharedShape {
    use cw_ui::font::{TextShape, TextShapeSource, align};
    use cw_ui::loader::{SceneShape, TextShapeNode};
    use cw_ui::shape::Keyed;
    let text: Vec<u16> = WAIT_TEXT.encode_utf16().collect();
    let src = TextShapeSource {
        strings: vec![text.clone()],
        size: 12.0,
        stroke_radius: if stroke { 3.0 } else { 0.0 },
        line_spacing: 3.0,
        flags: align::H_CENTER,
        font_file: "resource1.dat".into(),
        ..TextShapeSource::default()
    };
    let mut t = TextShapeNode {
        shape: TextShape::new(src),
        string: Keyed::from_frames(vec![text]),
        color: Keyed::from_frames(vec![[1.0; 4]]),
        stroke_color: Keyed::from_frames(vec![[0.0, 0.0, 0.0, 1.0]]),
        extrusion_color: Keyed::from_frames(vec![[0.0, 0.0, 0.0, 1.0]]),
        active: false,
    };
    t.sync();
    std::rc::Rc::new(std::cell::RefCell::new(SceneShape::Text(t)))
}

/// The loading label `GC+0x800898` (ctor 0x0045b576..0x0045b7d0): node `wait` with the
/// outline shape under the engine root (`0x0064f4e0(0, shape, 0, root = engine+0xb4
/// (0x00487490), L"wait")`, 0x0045b694), and a child `wait` with the fill shape
/// (0x0045b7c1). `onResize` 0x00482a40 centres it (0x00482d2c, `cw_ui::widget`), the flow
/// below shows it while the world loads, and the GUI root hides while it is up.
pub fn build_wait_label(gui: &mut cw_ui::widget::Gui, root: Option<NodeId>) -> NodeId {
    let text: Vec<u16> = WAIT_TEXT.encode_utf16().collect();
    let w = gui.add_plain_node(root, "wait");
    gui.nodes[w].text = Some(text.clone());
    gui.nodes[w].shape = Some(Box::new(wait_text_shape(true)));
    let f = gui.add_plain_node(Some(w), "wait");
    gui.nodes[f].text = Some(text);
    gui.nodes[f].shape = Some(Box::new(wait_text_shape(false)));
    w
}

/// `std::wstring` → `std::string` as 0x00659f50 narrows it (each UTF-16 unit truncated to a
/// byte; assumed, the function was not read).
pub fn narrow(s: &str) -> String {
    s.encode_utf16().map(|c| char::from(c as u8)).collect()
}

/// `wistringstream >> int` (the seed parse of 0x00482530): optional white space, sign,
/// digits; overflow saturates, no digits gives 0 (the VS2012 `num_get` behaviour).
pub fn parse_int(s: &str) -> i32 {
    let t = s.trim_start();
    let (neg, digits) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let d: String = digits.chars().take_while(|c| c.is_ascii_digit()).collect();
    if d.is_empty() {
        return 0;
    }
    match d.parse::<i64>() {
        Ok(v) => {
            let v = if neg { -v } else { v };
            v.clamp(i32::MIN as i64, i32::MAX as i64) as i32
        }
        Err(_) => {
            if neg {
                i32::MIN
            } else {
                i32::MAX
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Button handlers
// ---------------------------------------------------------------------------------------

/// Runs the GameController handler `h` (the `MemberFunctionConnection` target).
pub fn run_handler(ui: &mut GameUi, h: Handler, game: &mut GameView, out: &mut Vec<UiAction>) {
    match h {
        Handler::Back => back(ui),
        Handler::CreateCharacter => create_character(ui, game, out),
        Handler::CreateWorld => create_world(ui, game, out),
        Handler::ToggleMultiplayer => toggle_multiplayer(ui, game, out),
        Handler::ShowConnect => show_connect(ui),
        Handler::Connect => connect(ui, out),
        Handler::SelectCharacter => select_character(ui, game, out),
        Handler::SelectWorld => select_world(ui, game, out),
        Handler::DeleteCharacter => delete_character(game, out),
        Handler::DeleteWorld => delete_world(game, out),
        Handler::CharacterSmallButton => character_small_button(ui),
        Handler::WorldSmallButton => world_small_button(ui),
    }
}

/// The character screen's 20x20 button 0x004829c0 (placed by the carousel at the selected
/// preview's top-right corner): shows "Delete Character" (`GC+0x800998`). The carousel
/// hides it again when the selection changes or the "New character" entry is selected.
pub fn character_small_button(ui: &mut GameUi) {
    let d = ui.m.char_delete;
    ui.set_visible(d, true);
}

/// The world screen's 20x20 button 0x004829e0: shows "Delete World" (`GC+0x8009f0`).
pub fn world_small_button(ui: &mut GameUi) {
    let d = ui.m.world_delete;
    ui.set_visible(d, true);
}

/// `Node::setChildText(name, text, 1)` 0x00636a00: every node named `name` in `n`'s
/// subtree (the node itself included; a match is not searched further; nodes with flag bit 2
/// are skipped) gets `text` (`Node::setText` 0x00636ad0).
pub fn set_child_text(ui: &mut GameUi, n: Option<NodeId>, name: &str, text: &str) {
    let Some(n) = n else { return };
    if ui.gui.nodes[n].flags & cw_ui::widget::node_flags::DISABLED != 0 {
        return;
    }
    if ui.gui.nodes[n].name == name {
        ui.gui.nodes[n].text = Some(text.encode_utf16().collect());
        return;
    }
    for c in ui.gui.nodes[n].children.clone() {
        set_child_text(ui, Some(c), name, text);
    }
}

/// "Back" 0x004814f0: the first visible screen in this order is hidden and its parent shown.
pub fn back(ui: &mut GameUi) {
    let m = ui.m.clone();
    if ui.visible(m.world_create_root) {
        ui.set_visible(m.world_create_root, false);
        ui.set_visible(m.world_select_root, true);
    } else if ui.visible(m.world_select_root) {
        ui.set_visible(m.world_select_root, false);
        ui.set_visible(m.char_select_root, true);
    } else if ui.visible(m.char_create_root) {
        ui.set_visible(m.char_create_root, false);
        ui.set_visible(m.char_select_root, true);
    } else if ui.visible(m.char_select_root) {
        ui.set_visible(m.char_select_root, false);
        ui.set_visible(m.start_root, true);
    } else if ui.visible(m.server_root) {
        ui.set_visible(m.server_root, false);
        ui.set_visible(m.world_select_root, true);
    }
}

/// "Create Character" 0x004821a0.
pub fn create_character(ui: &mut GameUi, game: &mut GameView, out: &mut Vec<UiAction>) {
    let m = ui.m.clone();
    ui.set_visible(m.char_create_root, false);
    ui.set_visible(m.world_select_root, true);
    // The edit text, narrowed and copied to `creature+0x1168` (a strcpy; the creation
    // screen's rule keeps it at 2..15 characters), level (`creature+0x190`) = 1.
    let name = narrow(&edit_text(ui, m.character_name));
    let bytes = name.as_bytes();
    let n = bytes.len().min(15);
    game.player.0[0x1158..0x1168].fill(0);
    game.player.0[0x1158..0x1158 + n].copy_from_slice(&bytes[..n]);
    game.player.0[0x180..0x184].copy_from_slice(&1i32.to_le_bytes());
    // The new count is written to the database, the index is the old count.
    let index = game.characters.len() as i32;
    game.selected_character = index;
    game.characters.push(CharacterEntry {
        name: name.clone(),
        level: 1,
        class: game.player.0[0x130],
        specialization: game.player.0[0x131],
        entity_type: i32::from_le_bytes(game.player.0[0x54..0x58].try_into().unwrap()),
    });
    out.push(UiAction::SetName { name });
    out.push(UiAction::SaveCharacter { index });
    out.push(UiAction::RefreshPreviews);
}

/// "Create World" 0x00482530.
pub fn create_world(ui: &mut GameUi, game: &mut GameView, out: &mut Vec<UiAction>) {
    let m = ui.m.clone();
    ui.set_visible(m.world_create_root, false);
    out.push(UiAction::ResetCamera { yaw: 180.0 });
    let name = narrow(&edit_text(ui, m.world_name));
    let seed = parse_int(&edit_text(ui, m.world_seed));
    game.worlds.push(WorldEntry { name: name.clone(), seed, explored: 0 });
    let index = game.worlds.len() as i32 - 1;
    game.selected_world = index;
    out.push(UiAction::SaveWorld { index });
    out.push(UiAction::StartWorld { seed, name });
    out.push(UiAction::RefreshPreviews);
}

/// "Multiplayer Worlds..." 0x00484230: flips `GC+0x8009b0` and the button caption.
pub fn toggle_multiplayer(ui: &mut GameUi, game: &mut GameView, out: &mut Vec<UiAction>) {
    game.multiplayer = !game.multiplayer;
    let caption = if game.multiplayer { "Singleplayer worlds..." } else { "Multiplayer worlds..." };
    // `Node::setText` 0x00636ad0 on the button node.
    if let Some(n) = ui.m.multiplayer_worlds {
        super::present_hud::set_all_text(&mut ui.gui, n, caption);
    }
    out.push(UiAction::RefreshPreviews);
}

/// "Connect to server" 0x00481fe0: shows the server screen, hides the world list, then
/// `setChildText(L"edit", L"", 1)` 0x00636a00 on `serverName` (`GC+0x8009ac`, 0x00482089)
/// and on `connectionError` (`GC+0x8009f8`, 0x0048212b). The ctor gives `connectionError`
/// no `edit` child, so the second call only clears one if the `.plx` supplied it.
pub fn show_connect(ui: &mut GameUi) {
    let m = ui.m.clone();
    ui.set_visible(m.server_root, true);
    ui.set_visible(m.world_select_root, false);
    set_edit_text(ui, m.server_name, "");
    set_child_text(ui, m.connection_error, "edit", "");
}

/// "Connect" 0x004815e0: a non-empty address hides the screen and connects.
pub fn connect(ui: &mut GameUi, out: &mut Vec<UiAction>) {
    let m = ui.m.clone();
    let text = edit_text(ui, m.server_name);
    if !text.is_empty() {
        ui.set_visible(m.server_root, false);
        out.push(UiAction::Connect { address: narrow(&text) });
    }
}

/// Character "Select" 0x00483e70.
pub fn select_character(ui: &mut GameUi, game: &mut GameView, out: &mut Vec<UiAction>) {
    let m = ui.m.clone();
    ui.set_visible(m.char_select_root, false);
    let i = game.selected_character;
    if i < 0 || i >= game.characters.len() as i32 {
        // "New character": camera reset (yaw 0), the creation screen, a fresh player.
        out.push(UiAction::ResetCamera { yaw: 0.0 });
        ui.set_visible(m.char_create_root, true);
        out.push(UiAction::NewCharacter);
        set_edit_text(ui, m.character_name, "");
        // 0x0042bd90 then 0x0042c080(0): the style widget's defaults applied to the player.
        ui.char_style.reset();
        ui.char_style.apply(game);
    } else {
        out.push(UiAction::LoadCharacter { index: i });
        ui.set_visible(m.world_select_root, true);
    }
}

/// World "Select" 0x00484170.
pub fn select_world(ui: &mut GameUi, game: &mut GameView, out: &mut Vec<UiAction>) {
    let m = ui.m.clone();
    ui.set_visible(m.world_select_root, false);
    out.push(UiAction::ResetCamera { yaw: 180.0 });
    let i = game.selected_world;
    if i < game.worlds.len() as i32 {
        let w = &game.worlds[i as usize];
        out.push(UiAction::StartWorld { seed: w.seed, name: w.name.clone() });
        return;
    }
    ui.set_visible(m.world_create_root, true);
}

/// "Delete Character" 0x004816f0 (no bounds check on the index, like the original; an
/// out-of-range index does nothing here).
pub fn delete_character(game: &mut GameView, out: &mut Vec<UiAction>) {
    let i = game.selected_character;
    if i < 0 || i as usize >= game.characters.len() {
        return;
    }
    game.characters.remove(i as usize);
    out.push(UiAction::DeleteCharacter { index: i });
    out.push(UiAction::RefreshPreviews);
}

/// "Delete World" 0x00481d30. After the erase the selection is clamped with
/// `if (count < sel) sel = count - 1`, and every remaining world is rewritten with the
/// *selected* index (`0x004878a0(GC+0x800a10, world[i], 1)`, an original quirk kept here).
pub fn delete_world(game: &mut GameView, out: &mut Vec<UiAction>) {
    let i = game.selected_world;
    if i < 0 || i >= game.worlds.len() as i32 {
        return;
    }
    let name = game.worlds[i as usize].name.clone();
    out.push(UiAction::DeleteWorld { index: i, name });
    game.worlds.remove(i as usize);
    let count = game.worlds.len() as i32;
    if count < game.selected_world {
        game.selected_world = count - 1;
    }
    for _ in 0..count {
        out.push(UiAction::SaveWorld { index: game.selected_world });
    }
    out.push(UiAction::RefreshPreviews);
}

// ---------------------------------------------------------------------------------------
// Mouse (`onMouseDown` 0x0047b600, the UI parts)
// ---------------------------------------------------------------------------------------

/// The start-menu click at the top of `onMouseDown` 0x0047b600: with the start screen and
/// its menu visible and the left button, the hovered item (`StartMenuWidget+0x160`) runs:
/// 0 shows the character select, 1 the options panel, 2 sets the quit flag.
pub fn start_menu_click(ui: &mut GameUi, game: &mut GameView, button: i32, out: &mut Vec<UiAction>) {
    let m = ui.m.clone();
    if !(ui.visible(m.start_root) && ui.visible(m.start_menu_node) && button == 0) {
        return;
    }
    match ui.start_menu.hovered {
        0 => {
            ui.set_visible(m.start_root, false);
            ui.set_visible(m.char_select_root, true);
        }
        1 => ui.set_visible(m.options_panel, true),
        2 => {
            game.quit = true;
            out.push(UiAction::Quit);
        }
        _ => {}
    }
}

/// The system menu (`SystemWidget`, `GC+0x800914`) release, `onMouseUp` 0x0047ddd0 at
/// 0x0047e00e — the only reader of `SystemWidget+0x160`. **Call it from the mouse-up path**
/// (not mouse-down). With the system panel visible, the left button (`button == 0`) and a
/// hovered item (`+0x160 >= 0`): hides the panel, clears `GC+0x8008f2`, then
/// 0 "Options" shows the options panel (`GC+0x800a00`);
/// 1 "Start Menu" saves the selected character (`0x00487520(GC+0x800a0c, player)`) and the
/// selected world (`0x004878a0(GC+0x800a10, worlds[GC+0x800a10], 1)`, unchecked index),
/// disconnects (0x004719f0), reloads every character (0x004806c0), shows the start screen,
/// `startWorld(timeGetTime(), "")` and sets the start-menu camera;
/// 2 "Exit Game" sets the quit flag `GC+0x1a0`.
pub fn system_menu_click(ui: &mut GameUi, game: &mut GameView, button: i32, out: &mut Vec<UiAction>) {
    let m = ui.m.clone();
    if !(ui.visible(m.system_panel) && button == 0 && ui.system_menu_state.hovered >= 0) {
        return;
    }
    ui.set_visible(m.system_panel, false);
    ui.system_menu = false;
    match ui.system_menu_state.hovered {
        0 => ui.set_visible(m.options_panel, true),
        1 => {
            out.push(UiAction::SaveCharacter { index: game.selected_character });
            out.push(UiAction::SaveWorld { index: game.selected_world });
            out.push(UiAction::Disconnect);
            out.push(UiAction::ReloadCharacters);
            ui.set_visible(m.start_root, true);
            out.push(UiAction::StartWorldTimeSeed);
            out.push(UiAction::StartMenuCamera);
        }
        2 => {
            game.quit = true;
            out.push(UiAction::Quit);
        }
        _ => {}
    }
}

/// Right click on no widget (0x0047b711): closes the menus and the inventory, crafting and
/// customization panels.
pub fn right_click_nothing(ui: &mut GameUi) {
    let m = ui.m.clone();
    ui.flag_8008f0 = false;
    ui.skills_open = false;
    ui.system_menu = false;
    ui.set_visible(m.inventory_panel, false);
    ui.set_visible(m.crafting_panel, false);
    ui.set_visible(m.voxel_panel, false);
}

/// The menu-bar click (0x0047b7a5): with the system menu open and the left button, the
/// hovered menu button (`Button::isHovered` 0x006294c0) runs its action.
pub fn menu_bar_click(ui: &mut GameUi, game: &mut GameView, hovered: Option<usize>, button: i32) {
    if !(ui.system_menu && button == 0) {
        return;
    }
    match hovered {
        Some(0) => {
            let h = ui.m.help;
            let v = ui.visible(h);
            ui.set_visible(h, !v);
        }
        Some(1) => toggle_skills(ui, game),
        Some(2) => toggle_crafting(ui, game),
        Some(3) => toggle_inventory(ui),
        Some(4) => game.map_open = true,
        Some(5) => toggle_system(ui),
        _ => {}
    }
}

// ---------------------------------------------------------------------------------------
// Keys (`onKeyDown` 0x0047e1b0) and the panel toggles
// ---------------------------------------------------------------------------------------

/// B / I and menu button 3: 0x00488c00. Flips the inventory panel; opening it closes the
/// system panel. (0x004c6350 / 0x004c64c0 refresh the inventory widget.)
pub fn toggle_inventory(ui: &mut GameUi) {
    let m = ui.m.clone();
    let v = !ui.visible(m.inventory_panel);
    ui.set_visible(m.inventory_panel, v);
    if v {
        ui.set_visible(m.system_panel, false);
    }
}

/// C and menu button 2: 0x00488bd0. Flips the crafting panel, then rebuilds the recipe tabs
/// from the learned formulas and the bag (`0x004a14c0`, [`super::crafting::refresh`]).
pub fn toggle_crafting(ui: &mut GameUi, game: &GameView) {
    let m = ui.m.clone();
    let v = !ui.visible(m.crafting_panel);
    ui.set_visible(m.crafting_panel, v);
    let sel = (ui.inv.crafting.selected_tab, ui.inv.crafting.selected_index);
    super::crafting::refresh(&mut ui.crafting, game, sel);
}

/// O and menu button 5: 0x00488d00. Flips the system panel; opening it closes the inventory.
pub fn toggle_system(ui: &mut GameUi) {
    let m = ui.m.clone();
    let v = !ui.visible(m.system_panel);
    ui.set_visible(m.system_panel, v);
    if v {
        ui.set_visible(m.inventory_panel, false);
    }
}

/// X and menu button 1: 0x00488c70. Flips `GC+0x8008f1`; opening copies the player's eleven
/// skill levels (`creature+0x1138..`) and specialization (`+0x141`) into the skill widget and
/// hides the crafting and craft-preview panels.
pub fn toggle_skills(ui: &mut GameUi, game: &GameView) {
    ui.skills_open = !ui.skills_open;
    if ui.skills_open {
        ui.skills.load_from(&game.player);
        let m = ui.m.clone();
        ui.set_visible(m.crafting_panel, false);
        ui.set_visible(m.craft_preview_panel, false);
    }
}

/// The non-chat keys of `onKeyDown` 0x0047e1b0 that belong to the UI (virtual-key codes).
/// Returns whether the key was handled here. Blocked while a menu screen is visible or a
/// widget has keyboard focus (0x006531e0); Enter opens the chat input instead.
pub fn on_key_down(ui: &mut GameUi, game: &mut GameView, vk: u8) -> bool {
    let m = ui.m.clone();
    if ui.visible(m.char_create_root)
        || ui.visible(m.world_create_root)
        || ui.visible(m.char_select_root)
        || ui.visible(m.server_root)
        || ui.visible(m.world_select_root)
        || ui.gui.focused.is_some()
    {
        return false;
    }
    if ui.chat.input_active {
        return false;
    }
    match vk {
        0x0d => ui.chat.input_active = true,
        0x09 => ui.flag_800a40 = !ui.flag_800a40,
        0x1b => {
            game.map_open = false;
            ui.system_menu = !ui.system_menu;
        }
        0x42 | 0x49 => toggle_inventory(ui),
        0x43 => toggle_crafting(ui, game),
        0x4d => game.map_open = !game.map_open,
        0x4f => toggle_system(ui),
        0x56 => ui.flag_8007b4 = !ui.flag_8007b4,
        0x58 => toggle_skills(ui, game),
        0x70 => {
            let v = ui.visible(m.help);
            ui.set_visible(m.help, !v);
        }
        _ => return false,
    }
    true
}

// ---------------------------------------------------------------------------------------
// Per-frame rules (`update` 0x00488ee0)
// ---------------------------------------------------------------------------------------

/// `isCursorFree` 0x0047f1d0 (GameController slot 1): true when the chat input is active,
/// the map is open, one of the UI bytes is set, or any panel or menu screen is visible.
pub fn is_cursor_free(ui: &GameUi, game: &GameView) -> bool {
    let m = &ui.m;
    ui.chat.input_active
        || game.map_open
        || ui.flag_8008f0
        || ui.skills_open
        || ui.visible(m.system_panel)
        || ui.system_menu
        || ui.visible(m.inventory_panel)
        || ui.visible(m.shop_panel)
        || ui.visible(m.enchant_panel)
        || ui.visible(m.adaption_panel)
        || ui.visible(m.voxel_panel)
        || ui.visible(m.crafting_panel)
        || ui.visible(m.char_create_root)
        || ui.visible(m.char_select_root)
        || ui.visible(m.start_root)
        || ui.visible(m.world_select_root)
        || ui.visible(m.server_root)
        || ui.visible(m.world_create_root)
        || ui.visible(m.options_panel)
}

/// Whether the world-creation inputs are acceptable (update, Ghidra lines ~566..620): the
/// seed edit holds only `'0'..'9'`, the name is not an existing world's, and the name has
/// more than one character (`0x004348b0() > 1`).
fn world_creation_valid(name: &str, seed: &str, worlds: &[WorldEntry]) -> bool {
    if seed.chars().any(|c| !c.is_ascii_digit()) {
        return false;
    }
    if worlds.iter().any(|w| w.name == narrow(name)) {
        return false;
    }
    name.encode_utf16().count() > 1
}

/// The screen rules at the start of `update` 0x00488ee0 (0x00488f.. to 0x00489ca0). Emits
/// the menu-screen camera orbit when a select screen is up.
pub fn frame_rules(ui: &mut GameUi, game: &mut GameView, out: &mut Vec<UiAction>) {
    let m = ui.m.clone();
    // Connected without a socket: disconnect.
    if game.connected && !game.socket_open {
        out.push(UiAction::Disconnect);
    }
    // The options panel lives in the start screen while it is up, else in the GUI root;
    // the logo and the start menu hide while it is open.
    let start = ui.visible(m.start_root);
    if let Some(op) = m.options_panel {
        let want = if start { m.start_root } else { m.gui_root };
        if ui.gui.nodes[op].parent != want {
            reparent(ui, op, want);
        }
    }
    if start {
        let opts = ui.visible(m.options_panel);
        if m.cubeworld.is_some() {
            ui.set_visible(m.cubeworld, !opts);
        }
        ui.set_visible(m.start_menu_node, !opts);
    }
    // "Back" is up while any menu screen is.
    let any_menu = ui.visible(m.char_select_root)
        || ui.visible(m.char_create_root)
        || ui.visible(m.world_select_root)
        || ui.visible(m.world_create_root)
        || ui.visible(m.server_root);
    ui.set_visible(m.back, any_menu);
    // "Please wait...": the world is near but not meshed and no screen covers it, or the
    // world is pending. `load < 8 && load != 8` (a `comiss` pair: NaN is not "less").
    let wait = (game.load_distance < 8.0
        && !ui.visible(m.start_root)
        && !ui.visible(m.char_select_root)
        && !ui.visible(m.server_root)
        && !ui.visible(m.world_select_root))
        || game.world_pending;
    ui.set_visible(m.wait, wait);
    let ws = ui.visible(m.world_select_root);
    ui.set_visible(m.multiplayer_worlds, ws);
    ui.set_visible(m.connect_to_server, ws);
    let sv = ui.visible(m.server_root);
    ui.set_visible(m.connect, sv);
    // The GUI root is up unless a creation/select/server screen or the wait label is.
    let gui_up = !(ui.visible(m.char_create_root)
        || ui.visible(m.char_select_root)
        || ui.visible(m.server_root)
        || ui.visible(m.wait)
        || ui.visible(m.world_create_root));
    ui.set_visible(m.gui_root, gui_up);
    // Character creation: the name must be unique and 2..15 characters
    // (`len - 2 < 0xe`, unsigned), else "Create Character" hides; a duplicate shows
    // `nameError` "There is already a character with that name.".
    ui.set_visible(m.name_error, false);
    if ui.visible(m.char_create_root) {
        let name = edit_text(ui, m.character_name);
        let dup = game.characters.iter().any(|c| c.name == narrow(&name));
        let ok = if dup {
            ui.set_visible(m.name_error, true);
            if let Some(n) = m.name_error {
                ui.gui.nodes[n].text = Some("There is already a character with that name.".encode_utf16().collect());
            }
            false
        } else {
            (name.encode_utf16().count() as u32).wrapping_sub(2) < 0xe
        };
        ui.set_visible(m.create_character, ok);
    }
    // World creation.
    if ui.visible(m.world_create_root) {
        let name = edit_text(ui, m.world_name);
        let seed = edit_text(ui, m.world_seed);
        let ok = world_creation_valid(&name, &seed, &game.worlds);
        if !ok && game.worlds.iter().any(|w| w.name == narrow(&name)) {
            ui.set_visible(m.name_error, true);
            if let Some(n) = m.name_error {
                ui.gui.nodes[n].text = Some("There is already a world with that name.".encode_utf16().collect());
            }
        }
        ui.set_visible(m.create_world, ok);
    }
    // The camera orbit (distance 10, pitch 100, yaw += dt * 0.005), once per block that runs:
    // 0x00489cd3 `(start || server) && !wait`, 0x00489d4f `charSelect && !wait` and
    // 0x0048a818 `worldSelect && !wait` (the last two also run the preview carousels,
    // `previews::frame`). `wait` is the visibility set above.
    let wait = ui.visible(m.wait);
    if (ui.visible(m.start_root) || ui.visible(m.server_root)) && !wait {
        out.push(UiAction::OrbitCamera);
    }
    if ui.visible(m.char_select_root) && !wait {
        out.push(UiAction::OrbitCamera);
    }
    if ui.visible(m.world_select_root) && !wait {
        out.push(UiAction::OrbitCamera);
    }
    // Panels (0x0048b806, 0x0048c6c1, 0x0048d9b3, 0x0048d9f8, 0x0048eb4b).
    if ui.visible(m.crafting_panel) {
        ui.skills_open = false;
    }
    ui.flag_8008f0 = ui.visible(m.inventory_panel);
    let inv = ui.flag_8008f0;
    if let Some(p) = m.character.and_then(|w| ui.gui.parent_widget(w)) {
        let n = ui.gui.widgets[p].node;
        ui.gui.nodes[n].visible = inv;
    }
    // 0x0048c6ee..0x0048c7f6: each equipment box shown with the inventory and its `count`
    // texts cleared (`setChildText("count", "", 1)` 0x00636a00).
    for n in m.equipment_boxes.clone() {
        ui.gui.nodes[n].visible = inv;
        super::present_hud::set_named_text(&mut ui.gui, n, "count", "");
    }
    if ui.visible(m.start_root) {
        ui.skills_open = false;
        ui.set_visible(m.inventory_panel, false);
        ui.set_visible(m.adaption_panel, false);
        ui.set_visible(m.voxel_panel, false);
    }
    ui.set_visible(m.skills_panel, ui.skills_open);
    for n in m.menu_buttons.clone() {
        ui.gui.nodes[n].visible = ui.system_menu;
    }
    // The cursor follows isCursorFree (0x0048cbc3).
    let free = is_cursor_free(ui, game);
    ui.set_visible(m.cursor, free);
}

/// `Node::setParent` 0x00636950 (moves `n` to the end of `parent`'s children).
fn reparent(ui: &mut GameUi, n: NodeId, parent: Option<NodeId>) {
    if let Some(old) = ui.gui.nodes[n].parent {
        ui.gui.nodes[old].children.retain(|&c| c != n);
    }
    ui.gui.nodes[n].parent = parent;
    if let Some(p) = parent {
        ui.gui.nodes[p].children.push(n);
    }
}

#[cfg(test)]
mod tests {
    use super::super::members::NoPlx;
    use super::*;
    use cw_ui::widget::{Gui, UiEvent, event};

    fn setup() -> (GameUi, GameView) {
        let game = GameView::default();
        let ui = GameUi::new(Gui::new(), &mut NoPlx, &game);
        (ui, game)
    }

    fn press(ui: &mut GameUi, game: &mut GameView, n: Option<NodeId>) -> Vec<UiAction> {
        let w = ui.gui.nodes[n.unwrap()].widget.unwrap();
        ui.apply(&[UiEvent::Signal { widget: w, event: event::LEFT_RELEASE }], game)
    }

    /// The ctor's "Please wait..." label (0x0045b576..0x0045b7d0) is two TextShapes like the
    /// landscape banner's: `wait` (`GC+0x800898`, size 12, black stroke 3) under the engine
    /// root and its fill child `wait` (size 12, no stroke), both centred, line spacing 3.
    #[test]
    fn wait_label_has_text_shapes() {
        use cw_ui::loader::{SceneShape, SharedShape};
        let (ui, _) = setup();
        let w = ui.m.wait.unwrap();
        let fill = *ui.gui.nodes[w].children.first().expect("fill child");
        assert_eq!(ui.gui.nodes[fill].name, "wait");
        for (n, stroke) in [(w, 3.0), (fill, 0.0)] {
            let sh = ui.gui.nodes[n].shape.as_ref().and_then(|s| s.as_any()).and_then(|a| a.downcast_ref::<SharedShape>()).expect("TextShape");
            let SceneShape::Text(t) = &*sh.borrow() else { panic!("not a TextShape") };
            assert_eq!(String::from_utf16_lossy(&t.string.current), "Please wait...");
            assert_eq!(t.shape.source.size, 12.0);
            assert_eq!(t.shape.source.stroke_radius, stroke);
            assert_eq!(t.shape.source.line_spacing, 3.0);
            assert_eq!(t.shape.source.flags, cw_ui::font::align::H_CENTER);
            assert_eq!(t.shape.source.font_file, "resource1.dat");
        }
    }

    #[test]
    fn menu_flow_state_machine() {
        let (mut ui, mut game) = setup();
        let m = ui.m.clone();
        let mut out = Vec::new();
        // Start screen: "Start Game".
        ui.start_menu.hovered = 0;
        start_menu_click(&mut ui, &mut game, 0, &mut out);
        assert!(!ui.visible(m.start_root) && ui.visible(m.char_select_root));
        // No characters: Select opens the creation screen.
        let a = press(&mut ui, &mut game, m.char_select);
        assert!(a.contains(&UiAction::NewCharacter));
        assert!(ui.visible(m.char_create_root) && !ui.visible(m.char_select_root));
        // A one-letter name hides "Create Character"; a valid one shows it.
        set_edit_text(&mut ui, m.character_name, "A");
        frame_rules(&mut ui, &mut game, &mut out);
        assert!(!ui.visible(m.create_character));
        set_edit_text(&mut ui, m.character_name, "Wollay");
        frame_rules(&mut ui, &mut game, &mut out);
        assert!(ui.visible(m.create_character));
        let a = press(&mut ui, &mut game, m.create_character);
        assert!(a.contains(&UiAction::SaveCharacter { index: 0 }));
        assert_eq!(game.characters[0].name, "Wollay");
        assert_eq!(&game.player.0[0x1158..0x115e], b"Wollay");
        assert!(ui.visible(m.world_select_root) && !ui.visible(m.char_create_root));
        // Back from the world list goes to the character list, Back again to the start.
        press(&mut ui, &mut game, m.back);
        assert!(ui.visible(m.char_select_root) && !ui.visible(m.world_select_root));
        // Select the existing character: world list.
        game.selected_character = 0;
        let a = press(&mut ui, &mut game, m.char_select);
        assert_eq!(a, vec![UiAction::LoadCharacter { index: 0 }]);
        assert!(ui.visible(m.world_select_root));
        // No worlds: Select opens world creation.
        let a = press(&mut ui, &mut game, m.world_select);
        assert_eq!(a, vec![UiAction::ResetCamera { yaw: 180.0 }]);
        assert!(ui.visible(m.world_create_root));
        // A non-numeric seed blocks "Create World".
        set_edit_text(&mut ui, m.world_name, "Home");
        set_edit_text(&mut ui, m.world_seed, "12a");
        frame_rules(&mut ui, &mut game, &mut out);
        assert!(!ui.visible(m.create_world));
        set_edit_text(&mut ui, m.world_seed, "1234");
        frame_rules(&mut ui, &mut game, &mut out);
        assert!(ui.visible(m.create_world));
        let a = press(&mut ui, &mut game, m.create_world);
        assert!(a.contains(&UiAction::StartWorld { seed: 1234, name: "Home".into() }));
        assert!(!ui.visible(m.world_create_root));
        // In game: no menu screen, the GUI root is up, Back is hidden.
        game.load_distance = 20.0;
        frame_rules(&mut ui, &mut game, &mut out);
        assert!(ui.visible(m.gui_root) && !ui.visible(m.back) && !ui.visible(m.wait));
        // Options from the in-game side: toggles and the options panel's parent.
        toggle_system(&mut ui);
        assert!(ui.visible(m.system_panel));
        toggle_inventory(&mut ui);
        assert!(ui.visible(m.inventory_panel) && !ui.visible(m.system_panel));
        frame_rules(&mut ui, &mut game, &mut out);
        assert!(ui.flag_8008f0);
        assert!(is_cursor_free(&ui, &game));
        right_click_nothing(&mut ui);
        assert!(!ui.visible(m.inventory_panel));
        assert_eq!(ui.gui.nodes[m.options_panel.unwrap()].parent, m.gui_root);
    }

    #[test]
    fn server_connect_flow_and_back() {
        let (mut ui, mut game) = setup();
        let m = ui.m.clone();
        ui.set_visible(m.start_root, false);
        ui.set_visible(m.world_select_root, true);
        press(&mut ui, &mut game, m.connect_to_server);
        assert!(ui.visible(m.server_root) && !ui.visible(m.world_select_root));
        // Empty address: nothing happens.
        assert!(press(&mut ui, &mut game, m.connect).is_empty());
        set_edit_text(&mut ui, m.server_name, "127.0.0.1");
        let a = press(&mut ui, &mut game, m.connect);
        assert_eq!(a, vec![UiAction::Connect { address: "127.0.0.1".into() }]);
        assert!(!ui.visible(m.server_root));
        // Back from the server screen returns to the world list.
        ui.set_visible(m.server_root, true);
        back(&mut ui);
        assert!(ui.visible(m.world_select_root));
        // Multiplayer toggle.
        let a = press(&mut ui, &mut game, m.multiplayer_worlds);
        assert!(game.multiplayer && a == vec![UiAction::RefreshPreviews]);
    }

    #[test]
    fn delete_world_selection_quirk() {
        let mut game = GameView::default();
        game.worlds = vec![
            WorldEntry { name: "a".into(), seed: 1, explored: 0 },
            WorldEntry { name: "b".into(), seed: 2, explored: 0 },
            WorldEntry { name: "c".into(), seed: 3, explored: 0 },
        ];
        game.selected_world = 2;
        let mut out = Vec::new();
        delete_world(&mut game, &mut out);
        // count (2) < sel (2) is false: the selection stays on the "New world" slot.
        assert_eq!(game.selected_world, 2);
        assert_eq!(out.iter().filter(|a| matches!(a, UiAction::SaveWorld { index: 2 })).count(), 2);
    }

    #[test]
    fn small_buttons_show_delete_and_connect_clears() {
        let (mut ui, mut game) = setup();
        let m = ui.m.clone();
        assert!(!ui.visible(m.char_delete) && !ui.visible(m.world_delete));
        press(&mut ui, &mut game, m.char_small);
        assert!(ui.visible(m.char_delete));
        press(&mut ui, &mut game, m.world_small);
        assert!(ui.visible(m.world_delete));
        // A plx-supplied `edit` under connectionError is cleared by "Connect to server".
        let e = ui.gui.add_plain_node(m.connection_error, "edit");
        ui.gui.nodes[e].text = Some("x".encode_utf16().collect());
        show_connect(&mut ui);
        assert_eq!(ui.gui.nodes[e].text.as_deref(), Some(&[][..]));
    }

    #[test]
    fn system_menu_release() {
        let (mut ui, mut game) = setup();
        let m = ui.m.clone();
        ui.set_visible(m.system_panel, true);
        ui.system_menu = true;
        ui.system_menu_state.hovered = -1;
        let mut out = Vec::new();
        system_menu_click(&mut ui, &mut game, 0, &mut out);
        assert!(ui.visible(m.system_panel) && out.is_empty());
        ui.system_menu_state.hovered = 1;
        system_menu_click(&mut ui, &mut game, 0, &mut out);
        assert!(!ui.visible(m.system_panel) && !ui.system_menu && ui.visible(m.start_root));
        assert_eq!(out[2], UiAction::Disconnect);
        assert!(out.contains(&UiAction::StartWorldTimeSeed) && out.contains(&UiAction::ReloadCharacters));
    }

    #[test]
    fn seed_parse() {
        assert_eq!(parse_int("1234"), 1234);
        assert_eq!(parse_int(""), 0);
        assert_eq!(parse_int("99999999999"), i32::MAX);
    }
}
