//! The widget tree of `cube::GameController::GameController` `Cube.exe 0x00459c40` (48 KB):
//! which nodes it creates or finds by name, which `cube::*Widget`s it builds on them, the
//! GameController member each one lands in, and the nine `MemberFunctionConnection`s to
//! GameController handlers (`connect<GameController>` 0x004576f0, event 3 = left release).
//!
//! # Construction order (ctor address ranges)
//!
//! | Range | Step |
//! |---|---|
//! | 0x0045a746..0x0045aa95 | unnamed roots under the engine root: `+0x800884` GUI, `+0x800880` character creation, `+0x800890` world creation, `+0x800888` character select, `+0x800894` server connect, `+0x80088c` world select (the five screens hidden), `+0x800874` start screen |
//! | 0x0045ab3e..0x0045bffd | `textFX` (+0x800750), `minimap` (+0x800754), the "Hyara Planes" `landscape`/`landscapedetail` banner (+0x800858/+0x80085c), `wait` "Please wait..." (+0x800898), `nameError` "Error" (+0x8009f4), `connectionError` (+0x8009f8, under the server screen), `info` (+0x800860/+0x800864) |
//! | 0x0045c01c..0x0045c11b | the "PlasmaXGraphics" key, the map overlay nodes +0x8008a0/+0x8008a4 and `MapOverlayWidget` (not stored) |
//! | 0x0045c12f..0x0045c252 | `gui.plx` and `quest-tag.plx` into the GUI root; `help.plx` into +0x80089c (hidden) |
//! | 0x0045c262..0x0045cb76 | finds in the GUI root: `questtag`, `smallquesttag`, `landname`, `blackwidget`, `itembox`, `wideitembox`, `itemselector`, `combopoints`, the templates `leftbutton` `rightbutton` `upbutton` `downbutton` `scrollbutton` `edit` `button` `button2` `smallbutton` (the templates taken out of the tree, `setParent(NULL)` 0x00636950), `preview`, `equippedpreview`; `start.plx` into the start root, `cubeworld`, `picroma`; `star1..4` cloned into the banner |
//! | 0x0045cb8d..0x0045cf30 | cloned `button`s: "Create Character" (+0x800970), "Create World" (+0x8009b4), "Back" (+0x80099c), "Multiplayer Worlds..." (+0x8009a0), "Connect to server" (+0x8009a4), "Connect" (+0x8009a8), each connected |
//! | 0x0045cf4b..0x0045dd8e | the four edit fields: labels `charactername` "Name" (+0x800974), `worldName` "Name" (+0x8009bc), `worldSeed` "Seed (must be a number)" (+0x8009b8), `serverName` "Server address" (+0x8009ac), each with an `edit` clone centred 10 px below |
//! | 0x0045ddb8..0x0045e0a5 | fonts `resource1.dat`/`resource2.dat` (quit flag on failure); `crosshair`, `zoomcrosshair`, `lockedenemy`, `lockedfriend` (+0x8008ac..+0x8008b8); the skill/menu button templates |
//! | 0x0045e107..0x0046006e | `data3.db`: about 100 skill and UI icons (`+0x800820` by mode, `+0x800828` by skill, `+0x800830` by class and specialization; `skill_panel`) |
//! | 0x00460234..0x004604f6 | the quick bar: 6 `menubutton`... clones in +0x8007fc labelled M1, M2, 1..4, their `cooldown` children in +0x800808 |
//! | 0x00460520..0x00460a.. | the background `SpriteWidget` (+0x800a18) and the six menu buttons (+0x800838, icons help/skills/crafting/inventory/worldmap/system, labels F1 X C B M O) |
//! | 0x00460b0d.. | 12 equipment boxes (+0x800a34 nodes, +0x800a28 `SpriteWidget`s) labelled Left Weapon .. Pet |
//! | ..0x00461d6b | HUD nodes: `experiencebar` (+0x8007c4..+0x8007d0), `ridingbar` (+0x8007d4), `manacubebar` (+0x8007d8), `questbar` (+0x8007dc/+0x8007e0), `lifebar` (+0x800768..+0x800774), `mpbar`/`chargebar` (+0x8007b8..+0x8007c0), the eight target life bars (+0x800790..+0x8007ac), `hpbar` (+0x8007e4/+0x8007e8), `castbar` (+0x8007ec/+0x8007f0), `staminabar` (+0x8007f4/+0x8007f8), `selector` (+0x8008e0) + `PreviewWidget` on `preview` (+0x8008e8), `speech` bubbles (`SpeechWidget` per slot), the dialog `SpeechWidget` (+0x800938 on +0x80093c, 400x400), `ObjectiveWidget` (+0x800948 on +0x800944), `StatisticsWidget` (+0x800950 on +0x80094c), `cursor.plx` (+0x8008a8, hidden) |
//! | 0x004633.. | the local creature (+0x8006d0, named "Wollay" until created) and its textures |
//! | 0x004645..0x00465340 | `StartMenuWidget` (+0x80091c on +0x800918, 200x100, in the start root); the panels, each a clone of `blackwidget` in the GUI root holding one game widget: see [`GcMembers`] |
//! | 0x004655.. | `Save/characters.db` and `Save/worlds.db`, the select/delete buttons of both select screens, the previews (0x0049d650, 0x004a23d0), the `ChatWidget` (+0x800a14 on +0x800ad8, 400x200) |
//!
//! Every panel is `blackwidget` cloned by `0x00636040` into the GUI root, sized, hidden
//! (`0x00411a90(0)`), with the game widget's node attached as its content (`0x00631460`).

use cw_ui::widget::{
    Connection, GameControllerWidgets, GameWidget, GameWidgetClass, Gui, NodeId, NodeSource, WidgetId,
    WidgetSource, WidgetSourceKind, event,
};
use glam::Vec2;

/// A GameController member function a button is connected to (`connect<GameController>`
/// 0x004576f0 with event 3). The addresses are the handlers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Handler {
    /// 0x004821a0 "Create Character".
    CreateCharacter,
    /// 0x00482530 "Create World".
    CreateWorld,
    /// 0x004814f0 "Back".
    Back,
    /// 0x00484230 "Multiplayer Worlds..." (toggles singleplayer/multiplayer).
    ToggleMultiplayer,
    /// 0x00481fe0 "Connect to server".
    ShowConnect,
    /// 0x004815e0 "Connect".
    Connect,
    /// 0x00483e70 character "Select".
    SelectCharacter,
    /// 0x004829c0 the 20x20 button of the character select screen (hidden; not read).
    CharacterSmallButton,
    /// 0x004816f0 "Delete Character".
    DeleteCharacter,
    /// 0x00484170 world "Select".
    SelectWorld,
    /// 0x004829e0 the 20x20 button of the world select screen (hidden; not read).
    WorldSmallButton,
    /// 0x00481d30 "Delete World".
    DeleteWorld,
}

impl Handler {
    /// The handler's address.
    pub fn address(self) -> u32 {
        match self {
            Handler::CreateCharacter => 0x004821a0,
            Handler::CreateWorld => 0x00482530,
            Handler::Back => 0x004814f0,
            Handler::ToggleMultiplayer => 0x00484230,
            Handler::ShowConnect => 0x00481fe0,
            Handler::Connect => 0x004815e0,
            Handler::SelectCharacter => 0x00483e70,
            Handler::CharacterSmallButton => 0x004829c0,
            Handler::DeleteCharacter => 0x004816f0,
            Handler::SelectWorld => 0x00484170,
            Handler::WorldSmallButton => 0x004829e0,
            Handler::DeleteWorld => 0x00481d30,
        }
    }
}

/// The part nodes an `InventoryWidget` owns: the ctor 0x004c1bb0 clones the templates it is
/// given into the widget's node (`Node::clone` 0x00636040 with the template's Transformation
/// and Display copied, 0x00636b70 / 0x006368e0), and `setTabs` 0x004c6140 attaches the tab
/// nodes. `None` when the template is missing (the ctor keeps the null pointer).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InventoryParts {
    /// `+0x16c`: the `itemselector` clone (crafting only), hidden (its Display visibility
    /// key set to 0, 0x004c1cd6).
    pub selector: Option<NodeId>,
    /// `+0x170`: the `upbutton` clone; its `upbutton` node's `LEFT_PRESS` (2) is connected to
    /// `scrollUp` 0x004c60f0 (0x004c1a10).
    pub up: Option<NodeId>,
    /// `+0x174`: the `downbutton` clone, `LEFT_PRESS` -> `scrollDown` 0x004c5a00.
    pub down: Option<NodeId>,
    /// `+0x178`: the `scrollbutton` clone, `MOUSE_MOVE` (0xc) -> `dragScroll` 0x004c5bb0.
    pub scroll: Option<NodeId>,
    /// `+0x17c`: the tab nodes (clones of the GUI root's `tab`, `Node::clone(0)` 0x006326d0,
    /// added to the widget's node by 0x00630be0), each `LEFT_PRESS` -> `switchTab` 0x004c5a60.
    pub tabs: Vec<NodeId>,
    /// The icon texture of each tab (`Texture` loaded by 0x00486a20 and set on the clone's
    /// shape through `+0x7ac`, 0x00467f10): `buy.png` / `buy-back.png` (0x00700c68 /
    /// 0x00700c70) for the shop, `inventory-equipment.png`, `inventory-items.png`,
    /// `inventory-ingredients.png`, `inventory-pets.png` (0x00700d9c, 0x00700db4,
    /// 0x00700d80, 0x00700dc8) for the bag, in tab order.
    pub tab_icons: Vec<&'static str>,
}

/// The GameController widget members (`GC+0x800750..+0x800ad8`) the UI reads or writes, named
/// from the ctor 0x00459c40. `None` until [`construct`] fills them (or when a named node is
/// missing from the loaded `.plx`, which the original does not check either).
#[derive(Clone, Debug, Default)]
pub struct GcMembers {
    // --- roots ---
    /// +0x800884: the in-game GUI root (`gui.plx`, `quest-tag.plx`).
    pub gui_root: Option<NodeId>,
    /// +0x800880: character creation screen (hidden at start).
    pub char_create_root: Option<NodeId>,
    /// +0x800890: world creation screen.
    pub world_create_root: Option<NodeId>,
    /// +0x800888: character select screen.
    pub char_select_root: Option<NodeId>,
    /// +0x800894: server connect screen.
    pub server_root: Option<NodeId>,
    /// +0x80088c: world select screen.
    pub world_select_root: Option<NodeId>,
    /// +0x800874: start screen (`start.plx`).
    pub start_root: Option<NodeId>,
    /// +0x80089c: the help overlay (`help.plx`, hidden; F1 and menu button 0 toggle it).
    pub help: Option<NodeId>,
    // --- start screen ---
    /// +0x800878 `cubeworld` logo.
    pub cubeworld: Option<NodeId>,
    /// +0x80087c `picroma`.
    pub picroma: Option<NodeId>,
    /// +0x800918: node of the `StartMenuWidget` (200x100, child of the start root).
    pub start_menu_node: Option<NodeId>,
    /// +0x80091c `StartMenuWidget`.
    pub start_menu: Option<WidgetId>,
    // --- labels and status ---
    /// +0x800750 `textFX`.
    pub text_fx: Option<NodeId>,
    /// +0x800754 `minimap`.
    pub minimap: Option<NodeId>,
    /// +0x800758 `landname`.
    pub landname: Option<NodeId>,
    /// +0x800858 `landscape` banner ("Hyara Planes"); the four stars are cloned into it.
    pub landscape: Option<NodeId>,
    /// +0x800860 `info` text node.
    pub info: Option<NodeId>,
    /// +0x800868 `questtag`, +0x80086c `smallquesttag`.
    pub questtag: Option<NodeId>,
    /// +0x80086c.
    pub smallquesttag: Option<NodeId>,
    /// +0x800898 `wait` ("Please wait...").
    pub wait: Option<NodeId>,
    /// +0x8009f4 `nameError`.
    pub name_error: Option<NodeId>,
    /// +0x8009f8 `connectionError` (under the server screen).
    pub connection_error: Option<NodeId>,
    /// +0x800ad0 `combopoints`.
    pub combopoints: Option<NodeId>,
    // --- templates and markers ---
    /// +0x8008c8 `blackwidget` (panel template, hidden).
    pub blackwidget: Option<NodeId>,
    /// +0x8008cc `itembox`, +0x8008d0 `wideitembox`, +0x8008d4 `itemselector` (hidden templates).
    pub itembox: Option<NodeId>,
    /// +0x8008d0.
    pub wideitembox: Option<NodeId>,
    /// +0x8008d4.
    pub itemselector: Option<NodeId>,
    /// +0x8008ac `crosshair`, +0x8008b0 `zoomcrosshair`, +0x8008b4 `lockedenemy`, +0x8008b8
    /// `lockedfriend`.
    pub crosshair: Option<NodeId>,
    /// +0x8008b0.
    pub zoomcrosshair: Option<NodeId>,
    /// +0x8008b4.
    pub lockedenemy: Option<NodeId>,
    /// +0x8008b8.
    pub lockedfriend: Option<NodeId>,
    /// +0x8008a8: the mouse cursor (`cursor.plx`).
    pub cursor: Option<NodeId>,
    /// +0x8008e0 `selector`, +0x8008e4 `preview` (+0x8008e8 its `PreviewWidget`), +0x8008ec
    /// `equippedpreview`.
    pub selector: Option<NodeId>,
    /// +0x8008e4.
    pub preview: Option<NodeId>,
    /// +0x8008e8.
    pub preview_widget: Option<WidgetId>,
    /// +0x8008ec.
    pub equipped_preview: Option<NodeId>,
    // --- menu-screen buttons and edits ---
    /// +0x800970 "Create Character" (300x40, in the creation screen).
    pub create_character: Option<NodeId>,
    /// +0x8009b4 "Create World" (300x40).
    pub create_world: Option<NodeId>,
    /// +0x80099c "Back" (200x40, top level).
    pub back: Option<NodeId>,
    /// +0x8009a0 "Multiplayer Worlds..." (300x40 at (40, 40)).
    pub multiplayer_worlds: Option<NodeId>,
    /// +0x8009a4 "Connect to server" (300x40).
    pub connect_to_server: Option<NodeId>,
    /// +0x8009a8 "Connect" (200x40).
    pub connect: Option<NodeId>,
    /// +0x800974 `charactername` label ("Name") holding an `edit`.
    pub character_name: Option<NodeId>,
    /// +0x8009bc `worldName`.
    pub world_name: Option<NodeId>,
    /// +0x8009b8 `worldSeed`.
    pub world_seed: Option<NodeId>,
    /// +0x8009ac `serverName`.
    pub server_name: Option<NodeId>,
    /// +0x800990 character "Select" (200x30), +0x800994 its 20x20 button, +0x800998 "Delete
    /// Character" (300x30); all in the character select screen.
    pub char_select: Option<NodeId>,
    /// +0x800994.
    pub char_small: Option<NodeId>,
    /// +0x800998.
    pub char_delete: Option<NodeId>,
    /// +0x8009e8 world "Select", +0x8009ec 20x20, +0x8009f0 "Delete World".
    pub world_select: Option<NodeId>,
    /// +0x8009ec.
    pub world_small: Option<NodeId>,
    /// +0x8009f0.
    pub world_delete: Option<NodeId>,
    // --- HUD ---
    /// +0x8007c4 `experiencebar` (+0x8007c8 its `bar`, +0x8007cc clone, +0x8007d0 its `bar`).
    pub experience_bar: Option<NodeId>,
    /// +0x8007d4 `ridingbar`.
    pub riding_bar: Option<NodeId>,
    /// +0x8007d8 `manacubebar`.
    pub manacube_bar: Option<NodeId>,
    /// +0x8007dc `questbar` (+0x8007e0 `bar`, hidden).
    pub quest_bar: Option<NodeId>,
    /// +0x800768 `lifebar` (+0x80076c `bar`, +0x800770 clone, +0x800774 `bar`).
    pub life_bar: Option<NodeId>,
    /// +0x8007b8 `mpbar` (+0x8007bc `bar`, +0x8007c0 `chargebar`).
    pub mp_bar: Option<NodeId>,
    /// +0x800790..+0x8007ac: enemy/friend/static/neutral life bars, large then `:small`.
    pub target_bars: [Option<NodeId>; 8],
    /// +0x8007e4 `hpbar` (+0x8007e8 `bar`).
    pub hp_bar: Option<NodeId>,
    /// +0x8007ec `castbar` (+0x8007f0 `bar`).
    pub cast_bar: Option<NodeId>,
    /// +0x8007f4 `staminabar` (+0x8007f8 `bar`).
    pub stamina_bar: Option<NodeId>,
    /// +0x8007fc..: the six quick-bar slots (M1, M2, 1, 2, 3, 4).
    pub quick_slots: Vec<NodeId>,
    /// +0x800808..: their `cooldown` children.
    pub quick_cooldowns: Vec<NodeId>,
    /// +0x800838..: the six menu buttons (help F1, skills X, crafting C, inventory B, map M,
    /// system O), in this order.
    pub menu_buttons: Vec<NodeId>,
    /// +0x800978..: the character preview nodes (`CharacterPreviewWidget`s built by
    /// 0x0049d650 when the previews are refreshed; empty after the ctor).
    pub character_previews: Vec<NodeId>,
    /// +0x800a34..: the 12 equipment boxes; +0x800a28 their `SpriteWidget`s.
    pub equipment_boxes: Vec<NodeId>,
    /// +0x800a28.
    pub equipment_sprites: Vec<WidgetId>,
    /// +0x800a18: the background `SpriteWidget`.
    pub background_sprite: Option<WidgetId>,
    // --- dialog widgets ---
    /// +0x80093c: node of the dialog `SpeechWidget` (400x400, x centred, y 300).
    pub speech_node: Option<NodeId>,
    /// +0x800938: the dialog `SpeechWidget`.
    pub speech: Option<WidgetId>,
    /// +0x800944 / +0x800948: `ObjectiveWidget` node / widget.
    pub objective_node: Option<NodeId>,
    /// +0x800948.
    pub objective: Option<WidgetId>,
    /// +0x80094c / +0x800950: `StatisticsWidget` node / widget.
    pub statistics_node: Option<NodeId>,
    /// +0x800950.
    pub statistics: Option<WidgetId>,
    /// +0x800ad8 / +0x800a14: `ChatWidget` node / widget (400x200).
    pub chat_node: Option<NodeId>,
    /// +0x800a14.
    pub chat: Option<WidgetId>,
    // --- panels (blackwidget clones in the GUI root) and their widgets ---
    /// +0x800960: `CharacterWidget` (character sheet); its panel is its parent widget
    /// (260x200; placed at (20, 340) by onResize).
    pub character: Option<WidgetId>,
    /// +0x8008c4: shop panel (350x568); +0x80095c its `InventoryWidget` (type 3, data at
    /// GC+0x800c0c, tabs buy/buy-back).
    pub shop_panel: Option<NodeId>,
    /// +0x80095c.
    pub shop: Option<WidgetId>,
    /// +0x8008c0: crafting panel (350x285); +0x800958 its `InventoryWidget` (type 2, data at
    /// GC+0x800adc, `itemselector`, recipe tabs).
    pub crafting_panel: Option<NodeId>,
    /// +0x800958.
    pub crafting: Option<WidgetId>,
    /// +0x800ad4: craft preview panel (350x330); +0x800964 its `BlueprintPreviewWidget`.
    pub craft_preview_panel: Option<NodeId>,
    /// +0x800964.
    pub craft_preview: Option<WidgetId>,
    /// `BlueprintPreviewWidget+0x298` / `+0x29c` / `+0x2a0`: its `itemshadow`, `craftbutton`
    /// and `craftbar` parts (template clones under the widget's node).
    pub craft_itemshadow: Option<NodeId>,
    /// See [`GcMembers::craft_itemshadow`].
    pub craft_button: Option<NodeId>,
    /// See [`GcMembers::craft_itemshadow`].
    pub craft_bar: Option<NodeId>,
    /// `EnchantWidget+0x170`: its `itemframe` part.
    pub enchant_itemframe: Option<NodeId>,
    /// `AdaptionWidget+0x170` / `+0x174`: its `itemframe` and `rightarrow` parts.
    pub adaption_itemframe: Option<NodeId>,
    /// See [`GcMembers::adaption_itemframe`].
    pub adaption_rightarrow: Option<NodeId>,
    /// +0x80096c: character style panel (300x330, in the creation screen); +0x800968 its
    /// `CharacterStyleWidget`.
    pub char_style_panel: Option<NodeId>,
    /// +0x800968.
    pub char_style: Option<WidgetId>,
    /// +0x800a00: options panel (450x440); +0x8009fc its `OptionsWidget`.
    pub options_panel: Option<NodeId>,
    /// +0x8009fc.
    pub options: Option<WidgetId>,
    /// +0x800910: system menu panel (150x120, O); +0x800914 its `SystemWidget`.
    pub system_panel: Option<NodeId>,
    /// +0x800914.
    pub system: Option<WidgetId>,
    /// +0x8008f8: identification panel (350x285); +0x8008fc its `EnchantWidget`.
    pub enchant_panel: Option<NodeId>,
    /// +0x8008fc.
    pub enchant: Option<WidgetId>,
    /// +0x800900: adaption panel (350x400); +0x800904 its `AdaptionWidget`.
    pub adaption_panel: Option<NodeId>,
    /// +0x800904.
    pub adaption: Option<WidgetId>,
    /// +0x800908: skills panel (300x460); +0x80090c its `SkillWidget`.
    pub skills_panel: Option<NodeId>,
    /// +0x80090c.
    pub skills: Option<WidgetId>,
    /// +0x8008dc: weapon customization panel (400x624); +0x8008f4 its `VoxelWidget`.
    pub voxel_panel: Option<NodeId>,
    /// +0x8008f4.
    pub voxel: Option<WidgetId>,
    /// +0x8008bc: inventory panel (400x285); +0x800954 its `InventoryWidget` (type 0,
    /// `itembox`, tabs ingredients/equipment/items/pets).
    pub inventory_panel: Option<NodeId>,
    /// +0x800954.
    pub inventory: Option<WidgetId>,
    /// +0x800850 / +0x800854: two `specializationbutton` clones (the skill panel's radio).
    pub spec_buttons: [Option<NodeId>; 2],
    /// +0x800844: the eleven skill buttons (clones in the GUI root of `skillbutton` for the
    /// skills 0, 2, 4, 6 and of `childskillbutton` for the others, 0x004655a3..0x00465604).
    pub skill_buttons: Vec<NodeId>,
    /// +0x800820: `std::map<int, Texture*>` attack mode → icon (`none.png` under 0), as the
    /// texture index in `gui.plx`'s scene ([`PlxLoader::add_texture`]).
    pub mode_icons: std::collections::BTreeMap<i32, i32>,
    /// +0x800828: skill index → icon (the six non-class skills 0..5).
    pub skill_icons: std::collections::BTreeMap<i32, i32>,
    /// +0x800830: (class, specialization) → icon.
    pub spec_icons: std::collections::BTreeMap<(u8, i32), i32>,
    /// The part nodes of the three `InventoryWidget`s (ctor 0x004c1bb0 and the tab lists of
    /// 0x004c6140): bag (`+0x800954`), crafting (`+0x800958`), shop (`+0x80095c`).
    pub bag_parts: InventoryParts,
    /// See [`GcMembers::bag_parts`].
    pub crafting_parts: InventoryParts,
    /// See [`GcMembers::bag_parts`].
    pub shop_parts: InventoryParts,
    /// Button → GameController handler (the event map of each button).
    pub connections: Vec<(WidgetId, Handler)>,
    /// `CharacterStyleWidget+0x164..+0x188`: the ten arrow buttons (`leftbutton` /
    /// `rightbutton` clones, ctor 0x00427ce0) with the callback their `LEFT_PRESS` (2) runs,
    /// then the hair-colour palette's hit node (callback 0x0042b8b0).
    pub style_arrows: Vec<(WidgetId, super::character_style::Arrow)>,
    /// `OptionsWidget+0x170..+0x1c4`: the 22 arrow buttons (ctor 0x004cf3c0) as
    /// `(widget, row of [`super::options_menu::ROWS`], right arrow)`, `LEFT_PRESS` (2).
    pub option_arrows: Vec<(WidgetId, usize, bool)>,
    /// `OptionsWidget+0x1c8 / +0x1cc / +0x1d0`: Apply / OK / Cancel (`button2` clones),
    /// `LEFT_PRESS` (2).
    pub option_buttons: Vec<(WidgetId, super::options_menu::OptionsButton)>,
}

/// Every GameController member the UI touches: `(offset, field, how the ctor makes it)`.
pub const MEMBER_MAP: &[(u32, &str, &str)] = &[
    (0x800710, "engine", "D3D9Engine* (ctor argument)"),
    (0x800750, "text_fx", "createNode 'textFX' under the engine root"),
    (0x800754, "minimap", "createNode 'minimap' under +0x800884"),
    (0x800758, "landname", "findNode 'landname' in the GUI root"),
    (0x800768, "life_bar", "findNode 'lifebar'; +0x80076c 'bar', +0x800770 clone, +0x800774 'bar'"),
    (0x800790, "target_bars[0..8]", "findNode enemy/friend/static/neutral lifebar (+ ':small'), hidden"),
    (0x8007b8, "mp_bar", "findNode 'mpbar'; +0x8007bc 'bar'; +0x8007c0 'chargebar'"),
    (0x8007c4, "experience_bar", "findNode 'experiencebar'; +0x8007c8 'bar', +0x8007cc clone, +0x8007d0 'bar'"),
    (0x8007d4, "riding_bar", "findNode 'ridingbar'"),
    (0x8007d8, "manacube_bar", "findNode 'manacubebar'"),
    (0x8007dc, "quest_bar", "findNode 'questbar'; +0x8007e0 'bar' (hidden)"),
    (0x8007e4, "hp_bar", "findNode 'hpbar'; +0x8007e8 'bar'"),
    (0x8007ec, "cast_bar", "findNode 'castbar'; +0x8007f0 'bar'"),
    (0x8007f4, "stamina_bar", "findNode 'staminabar'; +0x8007f8 'bar'"),
    (0x8007fc, "quick_slots", "vector: 6 abilitybutton clones M1 M2 1 2 3 4, then quickitembutton"),
    (0x800808, "quick_cooldowns", "vector: their 'cooldown' children"),
    (0x800838, "menu_buttons", "vector: 6 menu buttons (help, skills, crafting, inventory, map, system)"),
    (0x800820, "mode_icons", "std::map<int, Texture*>: attack mode -> icon (none.png at 0)"),
    (0x800828, "skill_icons", "std::map<int, Texture*>: skill index 0..5 -> icon"),
    (0x800830, "spec_icons", "std::map<(class, spec), Texture*>"),
    (0x800844, "skill_buttons", "vector: 11 clones of skillbutton (0, 2, 4, 6) / childskillbutton"),
    (0x800850, "spec_buttons[0]", "clone of 'specializationbutton'"),
    (0x800854, "spec_buttons[1]", "clone of 'specializationbutton'"),
    (0x800858, "landscape", "createNode 'landscape' ('Hyara Planes' text) under the GUI root"),
    (0x800860, "info", "createNode 'info' under the GUI root"),
    (0x800868, "questtag", "findNode 'questtag'"),
    (0x80086c, "smallquesttag", "findNode 'smallquesttag'"),
    (0x800874, "start_root", "createNode '' under the engine root; start.plx"),
    (0x800878, "cubeworld", "findNode 'cubeworld' in the start root"),
    (0x80087c, "picroma", "findNode 'picroma' in the start root"),
    (0x800880, "char_create_root", "createNode '', hidden"),
    (0x800884, "gui_root", "createNode ''; gui.plx, quest-tag.plx"),
    (0x800888, "char_select_root", "createNode '', hidden"),
    (0x80088c, "world_select_root", "createNode '', hidden"),
    (0x800890, "world_create_root", "createNode '', hidden"),
    (0x800894, "server_root", "createNode '', hidden"),
    (0x800898, "wait", "createNode 'wait' ('Please wait...')"),
    (0x80089c, "help", "createNode '' under the GUI root; help.plx; hidden"),
    (0x8008a8, "cursor", "createNode ''; cursor.plx; hidden"),
    (0x8008ac, "crosshair", "findNode 'crosshair'"),
    (0x8008b0, "zoomcrosshair", "findNode 'zoomcrosshair'"),
    (0x8008b4, "lockedenemy", "findNode 'lockedenemy' (hidden)"),
    (0x8008b8, "lockedfriend", "findNode 'lockedfriend' (hidden)"),
    (0x8008bc, "inventory_panel", "blackwidget clone 400x285; InventoryWidget type 0 (+0x800954)"),
    (0x8008c0, "crafting_panel", "blackwidget clone 350x285 at (500, 300); InventoryWidget type 2 (+0x800958)"),
    (0x8008c4, "shop_panel", "blackwidget clone 350x568; InventoryWidget type 3 (+0x80095c)"),
    (0x8008c8, "blackwidget", "findNode 'blackwidget' (hidden template)"),
    (0x8008cc, "itembox", "findNode 'itembox' (hidden)"),
    (0x8008d0, "wideitembox", "findNode 'wideitembox' (hidden)"),
    (0x8008d4, "itemselector", "findNode 'itemselector' (hidden)"),
    (0x8008dc, "voxel_panel", "blackwidget clone 400x624; VoxelWidget (+0x8008f4)"),
    (0x8008e0, "selector", "findNode 'selector'"),
    (0x8008e4, "preview", "findNode 'preview'; PreviewWidget +0x8008e8"),
    (0x8008ec, "equipped_preview", "findNode 'equippedpreview'; visible while an item of an equipped type is previewed (0x0048f17e)"),
    (0x8008f0, "(byte) ui flag", "cleared by right click on nothing; read by isCursorFree (writer not found)"),
    (0x8008f1, "(byte) skills open", "toggled by X / menu button 1 (0x00488c70)"),
    (0x8008f2, "(byte) system menu open", "toggled by Esc; shows the menu buttons"),
    (0x8008f8, "enchant_panel", "blackwidget clone 350x285; EnchantWidget (+0x8008fc)"),
    (0x800900, "adaption_panel", "blackwidget clone 350x400; AdaptionWidget (+0x800904)"),
    (0x800908, "skills_panel", "blackwidget clone 300x460; SkillWidget (+0x80090c)"),
    (0x800910, "system_panel", "blackwidget clone 150x120; SystemWidget (+0x800914)"),
    (0x800918, "start_menu_node", "createNode '' in the start root, 200x100; StartMenuWidget (+0x80091c)"),
    (0x800938, "speech", "SpeechWidget on +0x80093c (400x400)"),
    (0x800944, "objective_node", "createNode ''; ObjectiveWidget +0x800948"),
    (0x80094c, "statistics_node", "createNode ''; StatisticsWidget +0x800950"),
    (0x800960, "character", "CharacterWidget in a 260x200 blackwidget clone"),
    (0x80096c, "char_style_panel", "blackwidget clone in the creation screen, 300x330; CharacterStyleWidget (+0x800968)"),
    (0x800970, "create_character", "button clone 300x40 'Create Character' -> 0x004821a0"),
    (0x800974, "character_name", "createNode 'charactername' ('Name') with an 'edit' clone"),
    (0x800978, "(vector) character previews", "filled by 0x0049d650; centred by onResize"),
    (0x800984, "(vector) characters", "Save/characters.db"),
    (0x800990, "char_select", "button clone 200x30 'Select' -> 0x00483e70"),
    (0x800994, "char_small", "button clone 20x20 -> 0x004829c0 (hidden)"),
    (0x800998, "char_delete", "button clone 300x30 'Delete Character' -> 0x004816f0 (hidden)"),
    (0x80099c, "back", "button clone 200x40 'Back' at (40, 40) -> 0x004814f0"),
    (0x8009a0, "multiplayer_worlds", "button clone 300x40 at (40, 40) -> 0x00484230"),
    (0x8009a4, "connect_to_server", "button clone 300x40 at (40, 40) -> 0x00481fe0"),
    (0x8009a8, "connect", "button clone 200x40 at (40, 40) -> 0x004815e0"),
    (0x8009ac, "server_name", "createNode 'serverName' ('Server address') with an 'edit' clone"),
    (0x8009b0, "(byte) multiplayer", "toggled by 0x00484230"),
    (0x8009b4, "create_world", "button clone 300x40 'Create World' -> 0x00482530"),
    (0x8009b8, "world_seed", "createNode 'worldSeed' ('Seed (must be a number)') with an 'edit'"),
    (0x8009bc, "world_name", "createNode 'worldName' ('Name') with an 'edit'"),
    (0x8009dc, "(vector) worlds", "Save/worlds.db (cube::WorldInfo, 0x28 bytes)"),
    (0x8009e8, "world_select", "button clone 200x30 'Select' -> 0x00484170"),
    (0x8009ec, "world_small", "button clone 20x20 -> 0x004829e0 (hidden)"),
    (0x8009f0, "world_delete", "button clone 300x30 'Delete World' -> 0x00481d30 (hidden)"),
    (0x8009f4, "name_error", "createNode 'nameError' ('Error')"),
    (0x8009f8, "connection_error", "createNode 'connectionError' under the server screen"),
    (0x8009fc, "options", "OptionsWidget in +0x800a00 (450x440)"),
    (0x800a0c, "(int) selected character", "carousel index"),
    (0x800a10, "(int) selected world", "carousel index"),
    (0x800a14, "chat", "ChatWidget on +0x800ad8 (400x200)"),
    (0x800a18, "background_sprite", "SpriteWidget 'background'"),
    (0x800a28, "equipment_sprites", "vector: 12 SpriteWidgets"),
    (0x800a34, "equipment_boxes", "vector: 12 equipment box nodes"),
    (0x800a40, "(byte) quick-item menu", "toggled by Tab; the selector shows while set; closed by the gameplay (0x00491f16)"),
    (0x800a44, "(int) quick-item selection", "moved by A/D in the menu; wrapped into the quick-item list (0x0047ae10)"),
    (0x800ad0, "combopoints", "findNode 'combopoints'"),
    (0x800ad4, "craft_preview_panel", "blackwidget clone 350x330; BlueprintPreviewWidget (+0x800964)"),
];

/// The renames applied to `cw_ui::widget::GameControllerWidgets` (2026-09-23): `(offset,
/// former cw-ui name, what the ctor makes: the new name)`. Kept as the record of the guessed
/// names; both structs now use the new names.
pub const CW_UI_CORRECTIONS: &[(u32, &str, &str)] = &[
    (0x80093c, "n_80093c", "node of the dialog SpeechWidget +0x800938 (400x400): `speech_node`"),
    (0x800918, "n_800918 (\"inventory area\")", "StartMenuWidget node in the start root: `start_menu_node`"),
    (0x80096c, "n_80096c (\"crafting area\")", "CharacterStyleWidget panel in the creation screen: `char_style_panel`"),
    (0x800a00, "n_800a00 (\"crafting area\")", "OptionsWidget panel: `options_panel`"),
    (0x800910, "n_800910 (\"crafting area\")", "SystemWidget panel (O): `system_panel`"),
    (0x8008bc, "n_8008bc (\"character/world select area\")", "inventory panel (InventoryWidget type 0, B/I): `inventory_panel`"),
    (0x8008c0, "n_8008c0 (\"an InventoryWidget, tab\")", "crafting panel (InventoryWidget type 2, C): `crafting_panel`"),
    (0x8008c4, "n_8008c4 (\"an InventoryWidget, tab\")", "shop panel (InventoryWidget type 3): `shop_panel`"),
    (0x800ad4, "n_800ad4 (\"an InventoryWidget, tab\")", "craft preview panel (BlueprintPreviewWidget): `craft_preview_panel`"),
    (0x8008f8, "n_8008f8 (\"crafting area\")", "identification panel (EnchantWidget): `enchant_panel`"),
    (0x800900, "n_800900 (\"crafting area\")", "adaption panel (AdaptionWidget): `adaption_panel`"),
    (0x800908, "n_800908 (\"crafting area\")", "skills panel (SkillWidget): `skills_panel`"),
    (0x8008dc, "n_8008dc (\"near Save/worlds.db\")", "weapon customization panel (VoxelWidget): `voxel_panel`"),
    (0x80089c, "n_80089c (\"top bar\")", "help overlay (help.plx, F1): `help`"),
    (0x800a14, "w_800a14 (\"near Delete World\")", "the ChatWidget: `chat`"),
    (0x800960, "w_800960", "CharacterWidget (its parent widget is the 260x200 panel): `character`"),
    (0x800938, "w_800938", "the dialog SpeechWidget: `speech`"),
    (0x800944, "n_800944", "ObjectiveWidget node: `objective_node`"),
    (0x80094c, "n_80094c", "StatisticsWidget node: `statistics_node`"),
    (0x800978, "centred", "character preview nodes (0x0049d650)"),
    (0x8007fc, "row_24", "quick-bar slots (M1 M2 1 2 3 4)"),
    (0x800838, "row_30", "menu buttons (F1 X C B M O)"),
    (0x800a34, "grid_2col", "the 12 equipment boxes"),
];

/// The `.plx` loading the constructor delegates to `Engine::loadFile` 0x00653770 (the
/// integrator maps cw-formats' reader output into the Gui under `parent`).
pub trait PlxLoader {
    /// Loads `file` (`gui.plx`, `quest-tag.plx`, `help.plx`, `start.plx`, `cursor.plx`) as
    /// children of `parent`.
    fn load(&mut self, gui: &mut Gui, file: &str, parent: NodeId);

    /// The game folder the files come from, when there is one: the GameController ctor
    /// also builds the `cube::World` (0x0058eb00), whose `cube::Speech` (0x004e1970) reads
    /// `data4.db` from the same working directory.
    fn game_dir(&self) -> Option<std::path::PathBuf> {
        None
    }

    /// `0x00486a20(name, 1)`: a texture read from `data3.db` (`0x004498d0`) and created from
    /// the PNG bytes (`Engine` 0x00658fa0), then given the format record `(1, 0, 0, 1, 1)`
    /// (0x004664e0 / 0x006612d0: pixel format 1, point filtering, wrap). The port appends it
    /// to the texture table of the scene loaded from `file` and returns its index there (the
    /// value a shape's `texture` attribute holds); `None` when it cannot be read.
    fn add_texture(&mut self, _file: &str, _name: &str) -> Option<i32> {
        None
    }

    /// `Node::clone` 0x006326d0 / 0x00636040 copy the template's Transformation and Display
    /// (0x00636b70 / 0x006368e0) and clone its shape: registers each `(template, clone)` pair
    /// of [`take_clone_log`] with the scene the template came from, so the renderer finds the
    /// clone's Display (fill colour, blur) and its scene's textures.
    fn register_clones(&mut self, _gui: &Gui, _pairs: &[(NodeId, NodeId)]) {}
}

thread_local! {
    /// Every `(template node, clone node)` pair [`clone_subtree`] made on this thread since the
    /// last [`take_clone_log`] (the whole subtree, parents first).
    static CLONE_LOG: std::cell::RefCell<Vec<(NodeId, NodeId)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The clone pairs recorded since the last call (see [`PlxLoader::register_clones`]). The
/// ctor drains it at its end; the controller should drain it after every UI frame (the select
/// screens clone `blackwidget` at run time) and hand it to `GamePlxLoader::register_clones`.
pub fn take_clone_log() -> Vec<(NodeId, NodeId)> {
    CLONE_LOG.with(|l| std::mem::take(&mut *l.borrow_mut()))
}

/// A loader that loads nothing (tests; the named nodes are then missing).
pub struct NoPlx;

impl PlxLoader for NoPlx {
    fn load(&mut self, _gui: &mut Gui, _file: &str, _parent: NodeId) {}
}

/// `Node::findNode` 0x00633d70: the first node named `name` in `root`'s subtree, depth first,
/// the root itself included (assumed order; the function was not read).
pub fn find_node(gui: &Gui, root: NodeId, name: &str) -> Option<NodeId> {
    if gui.nodes[root].name == name {
        return Some(root);
    }
    for &c in &gui.nodes[root].children {
        if let Some(n) = find_node(gui, c, name) {
            return Some(n);
        }
    }
    None
}

/// `Node::clone(parent)` 0x006326d0: `createNode` 0x0064f4e0 under `parent` with copies of
/// the shape (Shape slot 13 `clone`, [`cw_ui::widget::HitShape::clone_shape`]), the
/// Transformation and the Display (slot 2 `clone`: translation, pivot, rotation,
/// deformation, visibility, clip) and the name; then the widget (Widget slot 40 `clone`
/// onto the new node); then every child whose `Node.flags` bit 2 (`DISABLED`, 4) is clear,
/// recursively; then `Node.flags` (+0xc8) and the node's variable map (+0xe0/+0xe8, not
/// modelled). The `+0x44` owner-widget inheritance and the `Shape::rebuild(1)` of a shape
/// under a widget are the renderer's business.
pub fn clone_subtree(gui: &mut Gui, src: NodeId, parent: Option<NodeId>) -> NodeId {
    let s = &gui.nodes[src];
    let ns = NodeSource {
        name: s.name.clone(),
        flags: s.flags,
        translation: s.translation,
        pivot: s.pivot,
        rotation: s.rotation,
        deformation: Some(s.deformation),
        visible: s.visible,
        clip: s.clip,
        text: s.text.as_ref().map(|t| String::from_utf16_lossy(t)),
    };
    let shape = s.shape.as_ref().and_then(|sh| sh.clone_shape());
    let widget = s.widget;
    let children = s.children.clone();
    let n = gui.add_node(parent, ns);
    gui.nodes[n].shape = shape;
    CLONE_LOG.with(|l| l.borrow_mut().push((src, n)));
    if let Some(w) = widget {
        gui.clone_widget(w, n);
    }
    for c in children {
        // `(~(flags >> 2)) & 1`: disabled children are not cloned.
        if gui.nodes[c].flags & cw_ui::widget::node_flags::DISABLED == 0 {
            clone_subtree(gui, c, Some(n));
        }
    }
    n
}

/// One of the two TextShapes the ctor builds for an edit label (0x0045cf83..0x0045d069 and
/// 0x0045d105..0x0045d181, `TextShape` ctor 0x00663240 defaults otherwise): the outline
/// shape (`stroke`: stroke radius 3 in black) or the plain white one drawn over it.
fn label_text_shape(label: &str, stroke: bool) -> cw_ui::loader::SharedShape {
    use cw_ui::font::{TextShape, TextShapeSource, align};
    use cw_ui::loader::{SceneShape, TextShapeNode};
    use cw_ui::shape::Keyed;
    let text: Vec<u16> = label.encode_utf16().collect();
    let src = TextShapeSource {
        strings: vec![text.clone()],
        size: 12.0,
        stroke_radius: if stroke { 3.0 } else { 0.0 },
        line_spacing: 3.0,
        // 0x00487e80(1): `+0x1ec` flags.
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

/// `Node::updateWidgets` 0x00635700: a node with `Node.flags` bit 2 stops the walk; its
/// widget's slot 7 `update` runs, then the children in order. (Port: Edit widgets only, see
/// the call in [`construct`].)
pub fn update_widgets(gui: &mut Gui, n: NodeId) {
    if gui.nodes[n].flags & cw_ui::widget::node_flags::DISABLED != 0 {
        return;
    }
    if let Some(w) = gui.nodes[n].widget
        && matches!(gui.widgets[w].kind, cw_ui::widget::WidgetKind::Edit(_))
    {
        gui.update(w);
    }
    for c in gui.nodes[n].children.clone() {
        update_widgets(gui, c);
    }
}

fn hidden(gui: &mut Gui, parent: Option<NodeId>, name: &str) -> NodeId {
    gui.add_node(parent, NodeSource { name: name.into(), visible: false, ..Default::default() })
}

/// `Engine::deleteNode` 0x006504e0 (the ctor's template deletions): the node unlinked from
/// its parent; the arena keeps the slot, hidden and disabled.
fn delete_node(gui: &mut Gui, n: Option<NodeId>) {
    let Some(n) = n else { return };
    if let Some(p) = gui.nodes[n].parent.take() {
        gui.nodes[p].children.retain(|&c| c != n);
    }
    gui.nodes[n].visible = false;
    gui.nodes[n].flags |= cw_ui::widget::node_flags::DISABLED;
}

/// `Node::setParent(NULL)` 0x00636950: the node unlinked from its parent (the ctor's
/// template detaches; the port's hidden templates are the same thing for the renderer).
fn detach_node(gui: &mut Gui, n: Option<NodeId>) {
    let Some(n) = n else { return };
    if let Some(p) = gui.nodes[n].parent.take() {
        gui.nodes[p].children.retain(|&c| c != n);
    }
}

fn set_visible(gui: &mut Gui, n: Option<NodeId>, v: bool) {
    if let Some(n) = n {
        gui.nodes[n].visible = v;
    }
}

/// `0x0046eb90(0)`: `Node.flags |= 2` on `n` and every descendant.
fn set_no_events(gui: &mut Gui, n: NodeId) {
    gui.nodes[n].flags |= cw_ui::widget::node_flags::NO_EVENTS;
    for c in gui.nodes[n].children.clone() {
        set_no_events(gui, c);
    }
}

/// The hit shape of the style palette node (ctor 0x00427ce0: a four-corner shape (0, 0),
/// (280, 0), (280, 150), (0, 150) with transparent colours, made by 0x00650260). Only the
/// hit test uses it; the palette's cells are drawn by the widget's update body.
#[derive(Clone, Copy, Debug)]
struct RectShape(Vec2);

impl cw_ui::widget::HitShape for RectShape {
    fn contains_point(&self, p: Vec2) -> bool {
        0.0 <= p.x && 0.0 <= p.y && p.x < self.0.x && p.y < self.0.y
    }
    fn bounds(&self) -> (Vec2, Vec2) {
        (Vec2::ZERO, self.0)
    }
    fn clone_shape(&self) -> Option<Box<dyn cw_ui::widget::HitShape>> {
        Some(Box::new(*self))
    }
}

/// One arrow of the style or options widget: the template cloned under the widget's node
/// (`Node::clone` 0x00636040 with the template's Transformation and Display copied,
/// 0x00636b70 / 0x006368e0), then `connectByName(name, 2, widget, callback, 0, 1)`
/// (0x00427b40 / 0x004cf220): every widget named like the template in the clone gets the
/// `LEFT_PRESS` connection. Returns the clone and its (first) connected widget.
fn arrow_clone(gui: &mut Gui, template: Option<NodeId>, widget_node: NodeId, name: &str) -> Option<(NodeId, WidgetId)> {
    let n = clone_subtree(gui, template?, Some(widget_node));
    let mut found = None;
    let mut stack = vec![n];
    while let Some(k) = stack.pop() {
        if let Some(w) = gui.nodes[k].widget
            && gui.widgets[w].name == name
        {
            gui.connect(w, event::LEFT_PRESS, Connection { only_when_enabled: true });
            found.get_or_insert(w);
        }
        stack.extend(gui.nodes[k].children.iter().rev());
    }
    // A template whose widget carries another name: the clone's own widget (the port's
    // fallback; the shipped `leftbutton` / `rightbutton` widgets carry their node's name).
    let w = match found {
        Some(w) => w,
        None => {
            let w = gui.nodes[n].widget?;
            gui.connect(w, event::LEFT_PRESS, Connection { only_when_enabled: true });
            w
        }
    };
    Some((n, w))
}

/// `CharacterStyleWidget::CharacterStyleWidget` 0x00427ce0, the node part: the ten arrows
/// (`leftbutton`/`rightbutton` alternately, members `+0x164..+0x188`, callbacks in
/// [`super::character_style::ARROWS`] order) and the palette node (a transparent 280x150
/// shape at (10, 200) with a plain widget, `LEFT_PRESS` → 0x0042b8b0).
fn style_widget_parts(gui: &mut Gui, m: &mut GcMembers, w: WidgetId, left: Option<NodeId>, right: Option<NodeId>) {
    use super::character_style::Arrow;
    let wn = gui.widgets[w].node;
    let arrows = [
        (0x164, Arrow::RacePrev),
        (0x168, Arrow::RaceNext),
        (0x16c, Arrow::ClassPrev),
        (0x170, Arrow::ClassNext),
        (0x174, Arrow::Gender),
        (0x178, Arrow::Gender),
        (0x17c, Arrow::FacePrev),
        (0x180, Arrow::FaceNext),
        (0x184, Arrow::HairPrev),
        (0x188, Arrow::HairNext),
    ];
    for (i, (off, arrow)) in arrows.into_iter().enumerate() {
        let (t, name) = if i % 2 == 0 { (left, "leftbutton") } else { (right, "rightbutton") };
        if let Some((n, aw)) = arrow_clone(gui, t, wn, name) {
            set_game_child(gui, w, off, Some(n));
            m.style_arrows.push((aw, arrow));
        }
    }
    // `createNode(0, shape, 0, widgetNode, "")` 0x0064f4e0, a plain widget on it
    // (0x006503e0), connected (0x00427bc0, event 2), translated to (10, 200).
    let p = gui.add_plain_node(Some(wn), "");
    gui.nodes[p].shape = Some(Box::new(RectShape(Vec2::new(280.0, 150.0))));
    gui.nodes[p].translation = Vec2::new(10.0, 200.0);
    let pw = gui.add_widget(p, &WidgetSource::default());
    gui.connect(pw, event::LEFT_PRESS, Connection { only_when_enabled: true });
    m.style_arrows.push((pw, Arrow::PickColor));
}

/// `OptionsWidget::OptionsWidget` 0x004cf3c0, the node part: 22 arrows (`+0x170..+0x1c4`,
/// left/right per row of [`super::options_menu::ROWS`]) and three `button2` clones
/// (`Node::clone(widgetNode)` 0x006326d0) sized 110x15 (0x0062c570) with the texts "Apply",
/// "OK", "Cancel" (`Node::setText` 0x00636ad0), each `LEFT_PRESS` → 0x004d4470 / 0x004d4650 /
/// 0x004d4510 (0x004cf2a0).
fn options_widget_parts(gui: &mut Gui, m: &mut GcMembers, w: WidgetId, left: Option<NodeId>, right: Option<NodeId>, button2: Option<NodeId>) {
    use super::options_menu::OptionsButton;
    let wn = gui.widgets[w].node;
    for row in 0..super::options_menu::ROWS.len() {
        for (right_arrow, t, name) in [(false, left, "leftbutton"), (true, right, "rightbutton")] {
            if let Some((n, aw)) = arrow_clone(gui, t, wn, name) {
                let off = 0x170 + 8 * row as u32 + if right_arrow { 4 } else { 0 };
                set_game_child(gui, w, off, Some(n));
                m.option_arrows.push((aw, row, right_arrow));
            }
        }
    }
    for (off, caption, b) in [(0x1c8, "Apply", OptionsButton::Apply), (0x1cc, "OK", OptionsButton::Ok), (0x1d0, "Cancel", OptionsButton::Cancel)] {
        let Some(t) = button2 else { continue };
        let n = clone_subtree(gui, t, Some(wn));
        let Some(bw) = gui.nodes[n].widget else { continue };
        gui.set_size(bw, Vec2::new(110.0, 15.0), true);
        super::present_hud::set_all_text(gui, n, caption);
        gui.connect(bw, event::LEFT_PRESS, Connection { only_when_enabled: true });
        set_game_child(gui, w, off, Some(n));
        m.option_buttons.push((bw, b));
    }
}

/// A cloned `button` template (or a fresh push button when the template is missing) under
/// `parent`, sized `(w, h)`, captioned, and connected to `handler` on left release
/// (`connect<GameController>(3, gc, handler, 1, 1)`).
fn menu_button(
    gui: &mut Gui,
    m: &mut GcMembers,
    template: Option<NodeId>,
    parent: Option<NodeId>,
    size: Vec2,
    caption: &str,
    handler: Option<Handler>,
) -> NodeId {
    let n = match template {
        Some(t) => clone_subtree(gui, t, parent),
        None => gui.add_plain_node(parent, "button"),
    };
    gui.nodes[n].visible = true;
    let w = match gui.nodes[n].widget {
        Some(w) => w,
        None => gui.add_widget(n, &WidgetSource { kind: WidgetSourceKind::Button { button_type: 0 }, ..Default::default() }),
    };
    gui.set_size(w, size, true);
    // The ctor writes the caption with `Node::setText` 0x00636ad0 on the clone (every
    // TextShape of the subtree: the template's `text` nodes), not `Button::setCaption`
    // 0x0062b920 (which looks for nodes named `caption`, which these templates lack).
    if !caption.is_empty() {
        super::present_hud::set_all_text(gui, n, caption);
    }
    if let Some(h) = handler {
        gui.connect(w, event::LEFT_RELEASE, Connection { only_when_enabled: true });
        m.connections.push((w, h));
    }
    n
}

/// A game widget on a fresh parentless node (`createNode(0, 0, 0, 0, "")` then the widget
/// ctor), attached as the content of `panel` (0x00631460) when there is one.
fn game_widget(gui: &mut Gui, panel: Option<NodeId>, class: GameWidgetClass) -> (NodeId, WidgetId) {
    let n = gui.add_plain_node(panel, "");
    let w = gui.add_game_widget(n, &WidgetSource::default(), GameWidget::new(class));
    (n, w)
}

/// The end of `Node::attachWidget(name, widget, 1)` `Cube.exe 0x00631460` on a panel (the
/// ctor passes the `blackwidget` template's name, `0x0042b800` = `node + 0xc`, which every
/// panel clone carries): the widget's node is already a child of the panel (0x00630be0), and
/// with the flag set and a panel widget the game widget gets `setRect(0, 0, getSize(panel))`
/// (0x0062de60 / 0x0062baf0): it fills the panel, and its slot 10 `layout` runs with that size.
/// (The original also walks the panel's children for further `blackwidget` names; the panels
/// have none.)
pub(crate) fn attach_content(gui: &mut Gui, panel: Option<NodeId>, w: WidgetId) {
    let Some(pw) = panel.and_then(|p| gui.nodes[p].widget) else { return };
    let size = Vec2::new(gui.width(pw), gui.height(pw));
    gui.set_position_xy(w, 0.0, 0.0, true);
    gui.set_size(w, size, true);
}

/// `GameWidget::children` entry: the member at `offset` of the game widget (the part node its
/// ctor stored there), read by cw-ui's `layout` overrides.
fn set_game_child(gui: &mut Gui, w: WidgetId, offset: u32, n: Option<NodeId>) {
    let Some(n) = n else { return };
    if let cw_ui::widget::WidgetKind::Game(g) = &mut gui.widgets[w].kind {
        g.children.insert(offset, n);
    }
}

/// A game widget on a fresh node attached by `Node::attachWidget(name, widget, 1)`
/// 0x00631460: under the first node named `name` in `root`'s subtree (the port stops at the
/// first; the original visits every match), placed at (0, 0) with that node's widget size
/// (0x0062de60 / 0x0062baf0). Without a match (or a root) the widget's node stays
/// parentless, as the original's.
fn attached_game_widget(gui: &mut Gui, root: Option<NodeId>, name: &str, class: GameWidgetClass) -> (NodeId, WidgetId) {
    let target = root.and_then(|r| find_node(gui, r, name));
    let (n, w) = game_widget(gui, target, class);
    if let Some(tw) = target.and_then(|t| gui.nodes[t].widget) {
        let size = Vec2::new(gui.width(tw), gui.height(tw));
        gui.set_position_xy(w, 0.0, 0.0, true);
        gui.set_size(w, size, true);
    }
    (n, w)
}

/// A game widget's part node: the template `name` found in the GUI root (`findNode`
/// 0x00633d70), cloned under the widget's node (0x00636040, which copies the template's
/// Transformation and Display: 0x00636b70 / 0x006368e0). `None` when the template is missing
/// (the widget ctors then keep the null pointer).
fn widget_part(gui: &mut Gui, root: Option<NodeId>, widget_node: NodeId, name: &str) -> Option<NodeId> {
    let t = root.and_then(|r| find_node(gui, r, name))?;
    Some(clone_subtree(gui, t, Some(widget_node)))
}

/// The parts of `InventoryWidget::InventoryWidget` 0x004c1bb0 on the widget's node `wn`:
/// the templates cloned in ctor order (+0x16c selector first, then +0x170 up, +0x174 down,
/// +0x178 scroll), the selector hidden, the buttons' named nodes connected.
fn inventory_parts(
    gui: &mut Gui,
    wn: NodeId,
    selector: Option<NodeId>,
    up: Option<NodeId>,
    down: Option<NodeId>,
    scroll: Option<NodeId>,
) -> InventoryParts {
    let mut p = InventoryParts::default();
    if let Some(t) = selector {
        let n = clone_subtree(gui, t, Some(wn));
        // 0x004c1cd6: `display.visibility[current] = 0`.
        gui.nodes[n].visible = false;
        p.selector = Some(n);
    }
    // 0x004c1a10 `connectByName(node, name, event, widget, cb, 0, 1)`: the node named like the
    // template inside the clone (the clone itself when it carries the name).
    let part = |gui: &mut Gui, t: Option<NodeId>, name: &str, ev: u32| -> Option<NodeId> {
        let n = clone_subtree(gui, t?, Some(wn));
        if let Some(w) = find_node(gui, n, name).and_then(|b| gui.nodes[b].widget) {
            gui.connect(w, ev, Connection { only_when_enabled: true });
        }
        Some(n)
    };
    p.up = part(gui, up, "upbutton", event::LEFT_PRESS);
    p.down = part(gui, down, "downbutton", event::LEFT_PRESS);
    p.scroll = part(gui, scroll, "scrollbutton", event::MOUSE_MOVE);
    p
}

/// `InventoryWidget::setTabs(list)` 0x004c6140 at construction: every child of the widget's
/// node other than the four parts is deleted (0x006504e0; none exist yet), then each tab node
/// (a parentless `tab` clone, 0x006326d0 with parent 0) is added to the widget's node
/// (0x00630be0) at `x = 4`, then `x = (int)(width + 2 + x)` per tab (`__ftol2`), `y = -height
/// - 3` (0x0062a650, silent; width/height of the tab's widget, 0x0062f600 / 0x006291d0), and
/// its `LEFT_PRESS` (2) connected (0x004c1a90).
fn inventory_tabs(
    gui: &mut Gui,
    plx: &mut dyn PlxLoader,
    wn: NodeId,
    template: Option<NodeId>,
    icons: &[&'static str],
    parts: &mut InventoryParts,
) {
    let Some(t) = template else { return };
    let mut x: i32 = 4;
    for icon in icons {
        let n = clone_subtree(gui, t, None);
        // The icon texture on the clone's shape (the shape's `texture` attribute, `+0x7ac`
        // through 0x00467f10 for the bag and shop, `+0x7f8[+0x7cc]` in 0x004a1e50 for the
        // crafting tabs), then 0x0064ac00 refreshes the drawings' state.
        if let Some(tex) = plx.add_texture("gui.plx", icon) {
            set_shape_texture(gui, n, tex);
        }
        gui.add_child(wn, n);
        if let Some(w) = gui.nodes[n].widget {
            let y = -gui.height(w) - 3.0f32;
            gui.set_position_xy(w, x as f32, y, true);
            x = (gui.width(w) + 2.0f32 + x as f32) as i32;
            gui.connect(w, event::LEFT_PRESS, Connection { only_when_enabled: true });
        }
        parts.tabs.push(n);
        parts.tab_icons.push(icon);
    }
}

/// The InventoryWidget members cw-ui's `layout` (0x004c5d50) reads: `+0x168` the cell
/// template, `+0x16c` the selector, `+0x170` / `+0x174` the up / down buttons (placed at
/// `(w - 30, 10)` / `(w - 30, h - 30)`); `+0x178` (the scroll thumb) is placed by 0x004c64c0.
/// The selector's visibility is rewritten every frame by `present_panels::apply`.
fn inventory_members(gui: &mut Gui, w: WidgetId, cell: Option<NodeId>, p: &InventoryParts) {
    set_game_child(gui, w, 0x168, cell);
    set_game_child(gui, w, 0x16c, p.selector);
    let named = |gui: &Gui, n: Option<NodeId>, name: &str| n.map(|n| find_node(gui, n, name).unwrap_or(n));
    let up = named(gui, p.up, "upbutton");
    let down = named(gui, p.down, "downbutton");
    set_game_child(gui, w, 0x170, up);
    set_game_child(gui, w, 0x174, down);
    if let (Some(c), cw_ui::widget::WidgetKind::Game(g)) = (cell.and_then(|c| gui.nodes[c].widget), &gui.widgets[w].kind) {
        let _ = g;
        let size = glam::IVec2::new(super::inventory::cvtt(gui.width(c)), super::inventory::cvtt(gui.height(c)));
        if let cw_ui::widget::WidgetKind::Game(g) = &mut gui.widgets[w].kind {
            g.cell_size = size;
        }
    }
}

/// The cells of `InventoryWidget::layout` 0x004c5d50: `cols = (int)((w - 10) / (cellW + 5))`,
/// `rows = (int)((h - 40) / (cellH + 5))`; the old cells (children sharing the template's
/// shape, `+0x34`) are deleted, then per column and per row a clone of the cell template
/// (`Node::clone(0)` 0x00636040 with its Transformation copied) is put under the widget's node
/// (0x00635fe0) at `((cellW + 5) * col + 10, (cellH + 5) * row + 40)` (0x0062a650, silent).
fn inventory_cells(gui: &mut Gui, w: WidgetId, template: Option<NodeId>) {
    let Some(t) = template else { return };
    let wn = gui.widgets[w].node;
    let cell = match &gui.widgets[w].kind {
        cw_ui::widget::WidgetKind::Game(g) => g.cell_size,
        _ => return,
    };
    let cols = super::inventory::cvtt((gui.width(w) - 10.0f32) / (cell.x + 5) as f32);
    let rows = super::inventory::cvtt((gui.height(w) - 40.0f32) / (cell.y + 5) as f32);
    let old: Vec<NodeId> = gui.nodes[wn].children.iter().copied().filter(|&c| gui.nodes[c].name == gui.nodes[t].name).collect();
    for c in old {
        delete_node(gui, Some(c));
    }
    for col in 0..cols.max(0) {
        for row in 0..rows.max(0) {
            let c = clone_subtree(gui, t, Some(wn));
            let (x, y) = (((cell.x + 5) * col + 10) as f32, ((cell.y + 5) * row + 0x28) as f32);
            match gui.nodes[c].widget {
                Some(cw) => gui.set_position_xy(cw, x, y, true),
                None => gui.nodes[c].translation = Vec2::new(x, y),
            }
        }
    }
}

/// A `blackwidget` clone in the GUI root (0x00636040), sized, visible as given.
fn panel(gui: &mut Gui, m: &GcMembers, parent: Option<NodeId>, size: Vec2, visible: bool) -> NodeId {
    let n = match m.blackwidget {
        Some(t) => clone_subtree(gui, t, parent),
        None => gui.add_plain_node(parent, "blackwidget"),
    };
    gui.nodes[n].visible = visible;
    if gui.nodes[n].widget.is_none() {
        gui.add_widget(n, &WidgetSource::default());
    }
    let w = gui.nodes[n].widget.unwrap();
    gui.set_size(w, size, true);
    n
}

/// `GameController::GameController` 0x00459c40, the widget part, in the original order
/// (world, threads, sounds and textures are elsewhere). Positions are set later by
/// `onResize`.
pub fn construct(gui: &mut Gui, plx: &mut dyn PlxLoader) -> GcMembers {
    let mut m = GcMembers::default();
    let top = gui.root.unwrap_or_else(|| gui.add_plain_node(None, ""));
    // 0x0045a746..0x0045aa95: the roots. The five menu screens start hidden.
    m.gui_root = Some(gui.add_plain_node(Some(top), ""));
    m.char_create_root = Some(hidden(gui, Some(top), ""));
    m.world_create_root = Some(hidden(gui, Some(top), ""));
    m.char_select_root = Some(hidden(gui, Some(top), ""));
    m.server_root = Some(hidden(gui, Some(top), ""));
    m.world_select_root = Some(hidden(gui, Some(top), ""));
    m.start_root = Some(gui.add_plain_node(Some(top), ""));
    let g = m.gui_root;
    let (ccr, wcr, csr, wsr) = (m.char_create_root, m.world_create_root, m.char_select_root, m.world_select_root);
    // 0x0045ab3e..
    m.text_fx = Some(gui.add_plain_node(Some(top), "textFX"));
    m.minimap = Some(gui.add_plain_node(g, "minimap"));
    // 0x0045adae..0x0045b55a: the banner is four TextShapes (0x006502e0,
    // `super::hud::banner_text_shape`): `landscape` +0x800858 (size 20, outlined) under the
    // GUI root at (20, 20 + 5 + 20) (0x0045afce..0x0045b01f; `update` moves it), a child
    // `landscape` (size 20, the fill pass, shape +0x80085c), `landscapedetail` (size 12,
    // outlined) at (0, 20) (0x0045b3c7..0x0045b404) and its fill child `landscapedetail`.
    let banner = |gui: &mut Gui, parent: Option<NodeId>, name: &str, size: f32, stroke: bool| {
        let n = gui.add_plain_node(parent, name);
        gui.nodes[n].text = Some(super::hud::BANNER_TEXT.encode_utf16().collect());
        gui.nodes[n].shape = Some(Box::new(super::hud::banner_text_shape(size, stroke)));
        n
    };
    let landscape = banner(gui, g, "landscape", 20.0, true);
    gui.nodes[landscape].translation = Vec2::new(20.0, 20.0 + 5.0 + 20.0);
    banner(gui, Some(landscape), "landscape", 20.0, false);
    let detail = banner(gui, Some(landscape), "landscapedetail", 12.0, true);
    gui.nodes[detail].translation = Vec2::new(0.0, 20.0);
    banner(gui, Some(detail), "landscapedetail", 12.0, false);
    m.landscape = Some(landscape);
    // 0x0045b576..0x0045b7d0: the "Please wait..." label, outline and fill TextShapes.
    m.wait = Some(super::flow::build_wait_label(gui, Some(top)));
    let ne = gui.add_plain_node(Some(top), "nameError");
    gui.nodes[ne].text = Some("Error".encode_utf16().collect());
    m.name_error = Some(ne);
    m.connection_error = Some(gui.add_plain_node(m.server_root, "connectionError"));
    // 0x0045bd3f..0x0045c00c: `info` +0x800860 (size 12, right-aligned, outline 2) under the
    // GUI root at (20, 12 + 5 + 12) (`update` moves it to (W − 15, 20)), and a child `info`
    // with the fill shape +0x800864 (`super::hud::info_text_shape`).
    let info = gui.add_plain_node(g, "info");
    gui.nodes[info].text = Some(Vec::new());
    gui.nodes[info].shape = Some(Box::new(super::hud::info_text_shape(true)));
    gui.nodes[info].translation = Vec2::new(20.0, 12.0 + 5.0 + 12.0);
    let info_fill = gui.add_plain_node(Some(info), "info");
    gui.nodes[info_fill].text = Some(Vec::new());
    gui.nodes[info_fill].shape = Some(Box::new(super::hud::info_text_shape(false)));
    m.info = Some(info);
    // 0x0045c12f..: the .plx files.
    plx.load(gui, "gui.plx", g.unwrap());
    plx.load(gui, "quest-tag.plx", g.unwrap());
    let help = hidden(gui, g, "");
    plx.load(gui, "help.plx", help);
    m.help = Some(help);
    let find = |gui: &Gui, name: &str| g.and_then(|g| find_node(gui, g, name));
    m.questtag = find(gui, "questtag");
    m.smallquesttag = find(gui, "smallquesttag");
    plx.load(gui, "start.plx", m.start_root.unwrap());
    m.cubeworld = m.start_root.and_then(|s| find_node(gui, s, "cubeworld"));
    m.picroma = m.start_root.and_then(|s| find_node(gui, s, "picroma"));
    // star1..4 cloned into the banner at (-20, -65) (0x0045c394..0x0045c593); `update` hides
    // them every frame (0x00494722..0x0049483a, `present_hud::apply`).
    for s in super::hud::LANDSCAPE_STARS {
        if let Some(t) = find(gui, s) {
            let c = clone_subtree(gui, t, Some(landscape));
            gui.nodes[c].translation = Vec2::new(-20.0, -65.0);
        }
    }
    m.landname = find(gui, "landname");
    // 0x0045c5d0..0x0045c6eb: the templates are taken out of the GUI root with
    // `Node::setParent(NULL)` 0x00636950 (not hidden: their clones keep the file's Display).
    m.blackwidget = find(gui, "blackwidget");
    detach_node(gui, m.blackwidget);
    m.itembox = find(gui, "itembox");
    detach_node(gui, m.itembox);
    m.wideitembox = find(gui, "wideitembox");
    detach_node(gui, m.wideitembox);
    m.itemselector = find(gui, "itemselector");
    detach_node(gui, m.itemselector);
    m.combopoints = find(gui, "combopoints");
    // 0x0045c72b..0x0045cb0a: the button templates, found then detached the same way.
    let mut templates = std::collections::BTreeMap::new();
    for t in [
        "leftbutton", "rightbutton", "upbutton", "downbutton", "scrollbutton", "edit", "button", "button2", "smallbutton",
    ] {
        let n = find(gui, t);
        detach_node(gui, n);
        templates.insert(t, n);
    }
    // 0x0045c90c..0x0045c986, 0x0045c9cc..0x0045ca44, 0x0045ca8c..0x0045cb04: for `button`,
    // `button2` and `smallbutton` (not the other templates), every child of the template gets
    // `0x0046eb90(0)`: `Node.flags |= 2` (`NO_EVENTS`) on it and its whole subtree. The
    // captions' `text` nodes carry a plain Widget of their own; without the flag the hit test
    // stops on them (their widget becomes the hovered owner, so the button never gets its
    // enter/leave states and a click on the caption is not the button's).
    for t in ["button", "button2", "smallbutton"] {
        if let Some(n) = templates[t] {
            for c in gui.nodes[n].children.clone() {
                set_no_events(gui, c);
            }
        }
    }
    let button = templates["button"];
    let button2 = templates["button2"];
    let smallbutton = templates["smallbutton"];
    let (leftbutton, rightbutton) = (templates["leftbutton"], templates["rightbutton"]);
    let edit = templates["edit"];
    m.preview = find(gui, "preview");
    m.equipped_preview = find(gui, "equippedpreview");
    // 0x0045cb8d..0x0045cf30: the six menu-screen buttons.
    m.create_character = Some(menu_button(
        gui, &mut m, button, ccr, Vec2::new(300.0, 40.0), "Create Character", Some(Handler::CreateCharacter),
    ));
    m.create_world = Some(menu_button(
        gui, &mut m, button, wcr, Vec2::new(300.0, 40.0), "Create World", Some(Handler::CreateWorld),
    ));
    m.back = Some(menu_button(gui, &mut m, button, Some(top), Vec2::new(200.0, 40.0), "Back", Some(Handler::Back)));
    m.multiplayer_worlds = Some(menu_button(
        gui, &mut m, button, Some(top), Vec2::new(300.0, 40.0), "Multiplayer Worlds...", Some(Handler::ToggleMultiplayer),
    ));
    m.connect_to_server = Some(menu_button(
        gui, &mut m, button, Some(top), Vec2::new(300.0, 40.0), "Connect to server", Some(Handler::ShowConnect),
    ));
    m.connect = Some(menu_button(gui, &mut m, button, Some(top), Vec2::new(200.0, 40.0), "Connect", Some(Handler::Connect)));
    // 0x0045cf4b..0x0045dd8e: labelled edit fields.
    // Each: 0x006502e0 a TextShape (colour white, stroke black, pixel size 12, stroke
    // radius 3, line spacing 3, flags 1 = centred, font `resource1.dat`, the label string)
    // on `createNode(0, shape, 0, screen, name)` 0x0064f4e0, a second TextShape (white, the
    // same size and string, no stroke) on a child node of the same name, then the `edit`
    // template cloned under the label (`Node::clone(label)` 0x006326d0) and its widget placed
    // at `(label.x - width * 0.5, label.y + 10)` (0x0062a650; the label is still at (0, 0)
    // here, onResize 0x00482a40 moves the label), then `setChildText(L"edit", L"", 1)`
    // 0x00636a00 (0x0045d09d..0x0045d2b4 for `charactername`).
    let labelled_edit = |gui: &mut Gui, parent: Option<NodeId>, name: &str, label: &str| {
        let n = gui.add_plain_node(parent, name);
        gui.nodes[n].text = Some(label.encode_utf16().collect());
        gui.nodes[n].shape = Some(Box::new(label_text_shape(label, true)));
        let fill = gui.add_plain_node(Some(n), name);
        gui.nodes[fill].text = Some(label.encode_utf16().collect());
        gui.nodes[fill].shape = Some(Box::new(label_text_shape(label, false)));
        let e = match edit {
            Some(t) => {
                let e = clone_subtree(gui, t, Some(n));
                gui.nodes[e].visible = true;
                gui.nodes[e].name = "edit".into();
                e
            }
            None => {
                let e = gui.add_plain_node(Some(n), "edit");
                gui.add_widget(e, &WidgetSource { kind: WidgetSourceKind::Edit, ..Default::default() });
                e
            }
        };
        if let Some(w) = gui.nodes[e].widget {
            let x = 0.0 - gui.width(w) * 0.5;
            gui.set_position_xy(w, x, 0.0 + 10.0, true);
        }
        super::present_hud::set_named_text(gui, n, "edit", "");
        n
    };
    m.character_name = Some(labelled_edit(gui, m.char_create_root, "charactername", "Name"));
    m.world_name = Some(labelled_edit(gui, m.world_create_root, "worldName", "Name"));
    m.world_seed = Some(labelled_edit(gui, m.world_create_root, "worldSeed", "Seed (must be a number)"));
    m.server_name = Some(labelled_edit(gui, m.server_root, "serverName", "Server address"));
    // 0x0045de62..: markers.
    m.crosshair = find(gui, "crosshair");
    m.zoomcrosshair = find(gui, "zoomcrosshair");
    m.lockedenemy = find(gui, "lockedenemy");
    set_visible(gui, m.lockedenemy, false);
    m.lockedfriend = find(gui, "lockedfriend");
    set_visible(gui, m.lockedfriend, false);
    // 0x0045df5d..0x0045e0b6: the button templates of the GUI root, kept in locals:
    // `abilitybutton`, `skillbutton`, `specializationbutton`, `childskillbutton`,
    // `quickitembutton`, `menubutton`.
    let ability_template = find(gui, "abilitybutton");
    let skill_template = find(gui, "skillbutton");
    let spec_template = find(gui, "specializationbutton");
    let child_skill_template = find(gui, "childskillbutton");
    let quick_template = find(gui, "quickitembutton");
    // 0x0045e0f8..0x0046006e: the skill and UI icons of `data3.db` (0x00486a20 each, stored in
    // three maps: `+0x800820` by attack mode, `+0x800828` by skill index, `+0x800830` by
    // (class, specialization)). The port caches a name loaded twice (firebolt, waterbolt,
    // swirl, salvo, beam), which the original loads again.
    let mut loaded: std::collections::BTreeMap<&str, Option<i32>> = std::collections::BTreeMap::new();
    let mut tex = |plx: &mut dyn PlxLoader, name: &'static str| *loaded.entry(name).or_insert_with(|| plx.add_texture("gui.plx", name));
    for &(mode, name) in super::skill_panel::MODE_ICONS {
        if let Some(t) = tex(plx, name) {
            m.mode_icons.insert(mode, t);
        }
    }
    for &(skill, name) in super::skill_panel::SKILL_ICONS {
        if let Some(t) = tex(plx, name) {
            m.skill_icons.insert(skill, t);
        }
    }
    for &(key, name) in super::skill_panel::SPEC_ICONS {
        if let Some(t) = tex(plx, name) {
            m.spec_icons.insert(key, t);
        }
    }
    // 0x00460234..0x004604fc: the quick bar: six `abilitybutton` clones in the GUI root
    // (0x006326d0), each with its `cooldown` child in +0x800808 and its `info` texts set
    // (0x00636a00, recursive) to M1, M2, then "1".."4" (i - 1); the `cooldown` node is
    // scaled to (1, 0). 0x00460502: the `quickitembutton` node is pushed as
    // the seventh slot ("Q" from the file), without a cooldown entry.
    for i in 0..6 {
        let label = match i {
            0 => "M1".to_string(),
            1 => "M2".to_string(),
            _ => (i - 1).to_string(),
        };
        let n = match ability_template {
            Some(t) => clone_subtree(gui, t, g),
            None => gui.add_plain_node(g, "abilitybutton"),
        };
        gui.nodes[n].visible = true;
        if let Some(c) = find_node(gui, n, "cooldown") {
            m.quick_cooldowns.push(c);
            // 0x004604ab..0x004604d6: the `cooldown` node's scale set to (1, 0).
            gui.nodes[c].deformation[5] = 0.0;
            gui.nodes[c].deform_dirty = true;
        }
        super::present_hud::set_named_text(gui, n, "info", &label);
        m.quick_slots.push(n);
    }
    if let Some(q) = quick_template {
        m.quick_slots.push(q);
    }
    // 0x00460520..0x004605bf: the background `SpriteWidget` (+0x800a18) attached to the
    // quick-item button as its "background" (0x00631460).
    let (_, bg) = attached_game_widget(gui, quick_template, "background", GameWidgetClass::Sprite);
    m.background_sprite = Some(bg);
    // The six menu buttons: help (F1), skills (X), crafting (C), inventory (B), map (M),
    // system (O) (the loop counter starts at -1, so the `default:` help case comes first).
    // 0x004605d6..0x00460964, per button: the icon (`0x00486a20`, `data3.db`) set as the
    // clone's shape texture (`+0x7ac`, 0x00467f10) and the label written with
    // `Node::setText` 0x00636ad0 (every text node of the clone).
    let menu_template = find(gui, "menubutton");
    for (icon, label) in [
        ("help.png", "F1"),
        ("skills.png", "x"),
        ("crafting.png", "C"),
        ("inventory.png", "B"),
        ("worldmap.png", "M"),
        ("system.png", "O"),
    ] {
        let n = match menu_template {
            Some(t) => clone_subtree(gui, t, g),
            None => gui.add_plain_node(g, "menubutton"),
        };
        if let Some(tex) = plx.add_texture("gui.plx", icon) {
            set_shape_texture(gui, n, tex);
        }
        super::present_hud::set_all_text(gui, n, label);
        m.menu_buttons.push(n);
    }
    // 0x00460994..0x00460b60: the 12 equipment boxes: a `SpriteWidget` each, the box a
    // clone of `equipmentboxleft` (odd index) or `equipmentboxright` (even index) in the
    // GUI root with the sprite attached as its "background" (0x00631460); then both
    // templates deleted (0x006504e0).
    let box_left = find(gui, "equipmentboxleft");
    let box_right = find(gui, "equipmentboxright");
    for i in 0..12 {
        let t = if i & 1 != 0 { box_left } else { box_right };
        let n = match t {
            Some(t) => clone_subtree(gui, t, g),
            None => gui.add_plain_node(g, ""),
        };
        let (_, w) = attached_game_widget(gui, Some(n), "background", GameWidgetClass::Sprite);
        m.equipment_boxes.push(n);
        m.equipment_sprites.push(w);
    }
    delete_node(gui, box_left);
    delete_node(gui, box_right);
    // 0x00460b65..0x00460e4e: each box's text (`Node::setText` 0x00636ad0).
    let labels = [
        "Left
Weapon", "Right
Weapon", "Left Ring", "Right Ring", "Neck", "Shoulder", "Chest", "Hands", "Feet", "Light",
        "Special", "Pet",
    ];
    for (i, l) in labels.iter().enumerate() {
        super::present_hud::set_all_text(gui, m.equipment_boxes[i], l);
    }
    // 0x00460e65 / 0x00460e76: the `abilitybutton` and `menubutton` templates deleted.
    delete_node(gui, ability_template);
    delete_node(gui, menu_template);
    // HUD nodes.
    m.experience_bar = find(gui, "experiencebar");
    m.riding_bar = find(gui, "ridingbar");
    m.manacube_bar = find(gui, "manacubebar");
    m.quest_bar = find(gui, "questbar");
    m.life_bar = find(gui, "lifebar");
    m.mp_bar = find(gui, "mpbar");
    for (i, name) in [
        "enemylifebar", "friendlifebar", "staticlifebar", "neutrallifebar", "enemylifebar:small", "friendlifebar:small",
        "staticlifebar:small", "neutrallifebar:small",
    ]
    .iter()
    .enumerate()
    {
        m.target_bars[i] = find(gui, name);
        set_visible(gui, m.target_bars[i], false);
    }
    m.hp_bar = find(gui, "hpbar");
    m.cast_bar = find(gui, "castbar");
    m.stamina_bar = find(gui, "staminabar");
    m.selector = find(gui, "selector");
    if let Some(p) = m.preview {
        m.preview_widget = Some(gui.add_game_widget(p, &WidgetSource::default(), GameWidget::new(GameWidgetClass::Preview)));
        gui.nodes[p].visible = false;
    }
    // The dialog speech widget (400x400).
    let (sn, sw) = game_widget(gui, g, GameWidgetClass::Speech);
    gui.set_size(sw, Vec2::new(400.0, 400.0), true);
    m.speech_node = Some(sn);
    m.speech = Some(sw);
    let (on, ow) = game_widget(gui, Some(top), GameWidgetClass::Objective);
    m.objective_node = Some(on);
    m.objective = Some(ow);
    let (tn, tw) = game_widget(gui, Some(top), GameWidgetClass::Statistics);
    m.statistics_node = Some(tn);
    m.statistics = Some(tw);
    let cursor = hidden(gui, Some(top), "");
    plx.load(gui, "cursor.plx", cursor);
    // 0x00461e29: the original creates the cursor node without a parent
    // (`createNode(0, 0, 0, "")` 0x0064f4e0), so the engine's hit test from the root never
    // meets it. The port keeps it under the engine root to draw it and takes it out of the
    // hit test (`Node.flags` bit 12, 0x00636560); otherwise it would be the hovered node
    // under the cursor and no button could be hovered.
    gui.nodes[cursor].flags |= cw_ui::widget::node_flags::NO_HIT;
    m.cursor = Some(cursor);
    // 0x004645..: the start menu (200x100) and the panels.
    let (smn, smw) = game_widget(gui, m.start_root, GameWidgetClass::StartMenu);
    gui.set_size(smw, Vec2::new(200.0, 100.0), true);
    m.start_menu_node = Some(smn);
    m.start_menu = Some(smw);
    let p = panel(gui, &m, g, Vec2::new(260.0, 200.0), true);
    let (_, cw) = game_widget(gui, Some(p), GameWidgetClass::Character);
    attach_content(gui, Some(p), cw);
    m.character = Some(cw);
    // 0x004640eb: the `tab` template of the GUI root.
    let tab = find(gui, "tab");
    let (up, down, scroll) = (templates["upbutton"], templates["downbutton"], templates["scrollbutton"]);
    // 0x00464139..0x0046437d: the shop panel (clone of `blackwidget` in the GUI root, hidden,
    // 350x568) and its InventoryWidget (type 3, cells `wideitembox`, no selector), then its
    // two tabs.
    let sp = panel(gui, &m, g, Vec2::new(350.0, 568.0), false);
    m.shop_panel = Some(sp);
    let (spn, spw) = game_widget(gui, Some(sp), GameWidgetClass::Inventory);
    m.shop = Some(spw);
    m.shop_parts = inventory_parts(gui, spn, None, up, down, scroll);
    inventory_members(gui, spw, m.wideitembox, &m.shop_parts);
    attach_content(gui, Some(sp), spw);
    inventory_tabs(gui, plx, spn, tab, &["buy.png", "buy-back.png"], &mut m.shop_parts);
    // 0x00464382..0x004644f7: the crafting panel (350x285) and its InventoryWidget (type 2,
    // `wideitembox`, the `itemselector`); its tabs come from the recipe refresh 0x004a1e50
    // (`craft-*.png`, 0x004a228b), not from here.
    let cp = panel(gui, &m, g, Vec2::new(350.0, 285.0), false);
    m.crafting_panel = Some(cp);
    let (cpn, cpw) = game_widget(gui, Some(cp), GameWidgetClass::Inventory);
    m.crafting = Some(cpw);
    m.crafting_parts = inventory_parts(gui, cpn, m.itemselector, up, down, scroll);
    inventory_members(gui, cpw, m.wideitembox, &m.crafting_parts);
    attach_content(gui, Some(cp), cpw);
    // 0x004644f7: `0x004a1e50`, the crafting widget's six tabs (`setTabs` 0x004c6140) with the
    // `craft-*.png` icons of the local player's class; at this point of the ctor the player
    // is the fresh creature of 0x00462145 (class 1), and nothing calls 0x004a1e50 again.
    let icons = super::crafting::crafting_tab_icons(1);
    inventory_tabs(gui, plx, cpn, tab, &icons, &mut m.crafting_parts);
    // 0x004644fc..0x0046477d: the craft preview panel `+0x800ad4`: a `blackwidget` clone
    // whose parent is the GUI root `+0x800884` (the `push [ebx+0x800884]` of 0x004644fc
    // before `Node::clone` 0x00636040), hidden (0x00411a90 with 0 is `setVisible`, not
    // `setParent`), 350x330, at (920, 300); the BlueprintPreviewWidget is attached to it
    // (0x00631460). The next clone, the character-style panel `+0x80096c`, is the one parented
    // to the character-creation root `+0x800880`.
    let bp = panel(gui, &m, g, Vec2::new(350.0, 330.0), false);
    m.craft_preview_panel = Some(bp);
    let (bpn, bpw) = game_widget(gui, Some(bp), GameWidgetClass::BlueprintPreview);
    m.craft_preview = Some(bpw);
    // ctor 0x0045.. / BlueprintPreviewWidget ctor 0x0042f190: the `itemshadow`, `craftbutton`
    // and `craftbar` templates of the GUI root, cloned into the widget's node (0x00636040,
    // the template's Transformation and Display copied, 0x00636b70 / 0x006368e0).
    // Correction (0x0042f190 decompiled): `itemshadow` is only stored (`+0x298`);
    // `craftbutton` (`+0x29c`) and `craftbar` (`+0x2a0`) are moved under the widget's node
    // (`Node::setParent` 0x00636950), not cloned.
    m.craft_itemshadow = find(gui, "itemshadow");
    for (slot, name) in [(&mut m.craft_button, "craftbutton"), (&mut m.craft_bar, "craftbar")] {
        *slot = find(gui, name);
        if let Some(n) = *slot {
            gui.add_child(bpn, n);
        }
    }
    // `BlueprintPreviewWidget::layout` 0x004348f0 runs when the attach 0x00631460 sizes the
    // widget to the panel (cw-ui `Gui::layout`, the geometry of `crafting::preview_layout`):
    // `itemshadow` moved under the widget's node (0x00635fe0) and centred at y 90, the craft
    // button in the bottom-right corner (20 px margins), the craft bar left of it.
    let _ = bpn;
    set_game_child(gui, bpw, 0x298, m.craft_itemshadow);
    set_game_child(gui, bpw, 0x29c, m.craft_button);
    set_game_child(gui, bpw, 0x2a0, m.craft_bar);
    attach_content(gui, Some(bp), bpw);
    // 0x004645e7..0x00464687: the craft button's icon. `gui.plx` gives the `craftbutton`
    // shape no texture (`SmoothMeshShape.texture` -1, white vertex colours: a plain white
    // square); the ctor finds the node in the GUI root (0x00633d70 "craftbutton") and writes
    // `GC+0x800820[0x39]` (the mode-0x39 icon, `slam.png`, 0x00468910 / 0x0047b5f0) into its
    // shape's `texture` attribute (`+0x7ac`, 0x00467f10), then 0x0064ac00.
    if let (Some(n), Some(&t)) = (find(gui, "craftbutton"), m.mode_icons.get(&0x39)) {
        set_shape_texture(gui, n, t);
    }
    // 0x00464782: the `tab` template taken out of the GUI root (`setParent(NULL)`
    // 0x00636950); the bag's tabs below are still cloned from it.
    detach_node(gui, tab);
    let csp = panel(gui, &m, m.char_create_root, Vec2::new(300.0, 330.0), true);
    m.char_style_panel = Some(csp);
    let csw = game_widget(gui, Some(csp), GameWidgetClass::CharacterStyle).1;
    // 0x0046487d: `CharacterStyleWidget(engine, node, gc, leftbutton, rightbutton)`.
    style_widget_parts(gui, &mut m, csw, leftbutton, rightbutton);
    attach_content(gui, Some(csp), csw);
    m.char_style = Some(csw);
    let op = panel(gui, &m, g, Vec2::new(450.0, 440.0), false);
    m.options_panel = Some(op);
    let ow = game_widget(gui, Some(op), GameWidgetClass::Options).1;
    // 0x004649ab: `OptionsWidget(engine, node, gc, leftbutton, rightbutton, button2)`.
    options_widget_parts(gui, &mut m, ow, leftbutton, rightbutton, button2);
    attach_content(gui, Some(op), ow);
    m.options = Some(ow);
    let syp = panel(gui, &m, g, Vec2::new(150.0, 120.0), false);
    m.system_panel = Some(syp);
    let syw = game_widget(gui, Some(syp), GameWidgetClass::System).1;
    attach_content(gui, Some(syp), syw);
    m.system = Some(syw);
    let ep = panel(gui, &m, g, Vec2::new(350.0, 285.0), false);
    m.enchant_panel = Some(ep);
    let (epn, epw) = game_widget(gui, Some(ep), GameWidgetClass::Enchant);
    m.enchant = Some(epw);
    // EnchantWidget ctor 0x0044e910: `+0x170` = the `itemframe` template cloned in.
    m.enchant_itemframe = widget_part(gui, g, epn, "itemframe");
    set_game_child(gui, epw, 0x170, m.enchant_itemframe);
    attach_content(gui, Some(ep), epw);
    let ap = panel(gui, &m, g, Vec2::new(350.0, 400.0), false);
    m.adaption_panel = Some(ap);
    let (apn, apw) = game_widget(gui, Some(ap), GameWidgetClass::Adaption);
    m.adaption = Some(apw);
    // AdaptionWidget ctor 0x0040ecd0: `+0x170` `itemframe`, `+0x174` `rightarrow`.
    m.adaption_itemframe = widget_part(gui, g, apn, "itemframe");
    m.adaption_rightarrow = widget_part(gui, g, apn, "rightarrow");
    set_game_child(gui, apw, 0x170, m.adaption_itemframe);
    set_game_child(gui, apw, 0x174, m.adaption_rightarrow);
    attach_content(gui, Some(ap), apw);
    // 0x00464eea / 0x00464ef7: the `itemframe` and `rightarrow` templates taken out of the
    // GUI root (`setParent(NULL)` 0x00636950).
    let (itemframe, rightarrow) = (find(gui, "itemframe"), find(gui, "rightarrow"));
    detach_node(gui, itemframe);
    detach_node(gui, rightarrow);
    let skp = panel(gui, &m, g, Vec2::new(300.0, 460.0), false);
    m.skills_panel = Some(skp);
    let skw = game_widget(gui, Some(skp), GameWidgetClass::Skill).1;
    attach_content(gui, Some(skp), skw);
    m.skills = Some(skw);
    let vp = panel(gui, &m, g, Vec2::new(400.0, 624.0), false);
    m.voxel_panel = Some(vp);
    let vw = game_widget(gui, Some(vp), GameWidgetClass::Voxel).1;
    attach_content(gui, Some(vp), vw);
    m.voxel = Some(vw);
    // 0x004652fa..0x0046553e: the inventory panel (400x285) and its InventoryWidget (type 0,
    // `itembox`, no selector), then its four tabs.
    let ip = panel(gui, &m, g, Vec2::new(400.0, 285.0), false);
    m.inventory_panel = Some(ip);
    let (ipn, ipw) = game_widget(gui, Some(ip), GameWidgetClass::Inventory);
    m.inventory = Some(ipw);
    m.bag_parts = inventory_parts(gui, ipn, None, up, down, scroll);
    inventory_members(gui, ipw, m.itembox, &m.bag_parts);
    attach_content(gui, Some(ip), ipw);
    inventory_tabs(
        gui,
        plx,
        ipn,
        tab,
        &["inventory-equipment.png", "inventory-items.png", "inventory-ingredients.png", "inventory-pets.png"],
        &mut m.bag_parts,
    );
    // The cell nodes of the three InventoryWidgets (`layout` 0x004c5d50). The attach's
    // layout made them, and `setTabs` 0x004c6140 deleted every child but the four parts
    // again; they come back with the widget's next layout (a later `setSize` /
    // `onParentResized` of the panel). Assumed to have run before the first frame is drawn.
    for (w, cell) in [(m.shop, m.wideitembox), (m.crafting, m.wideitembox), (m.inventory, m.itembox)] {
        if let Some(w) = w {
            inventory_cells(gui, w, cell);
        }
    }
    // The two specialization buttons.
    // 0x0046556b..0x0046559e: two clones of `specializationbutton` (+0x800850/+0x800854),
    // then the template deleted; 0x00465606..0x00465623: `skillbutton` and
    // `childskillbutton` deleted after the eleven skill buttons (+0x800844) were cloned.
    for i in 0..2 {
        m.spec_buttons[i] = spec_template.map(|t| clone_subtree(gui, t, g));
    }
    delete_node(gui, spec_template);
    // 0x004655a3..0x00465604: the eleven skill buttons (+0x800844), clones in the GUI root
    // (0x006326d0) of `skillbutton` for the root skills 0, 2, 4, 6 and of `childskillbutton`
    // for the others.
    for i in 0..11 {
        let t = if matches!(i, 0 | 2 | 4 | 6) { skill_template } else { child_skill_template };
        let n = match t {
            Some(t) => clone_subtree(gui, t, g),
            None => gui.add_plain_node(g, "skillbutton"),
        };
        m.skill_buttons.push(n);
    }
    delete_node(gui, skill_template);
    delete_node(gui, child_skill_template);
    // 0x00465628..: the select screens' buttons: "Select" is a `button2` clone
    // (`[ebp-0x6fb8]`), the 20x20 button a `smallbutton` clone (`[ebp-0x6f80]`, no text
    // written: it keeps the template's "x"), "Delete ..." a `button` clone (`[ebp-0x6fbc]`).
    m.char_select = Some(menu_button(
        gui, &mut m, button2, csr, Vec2::new(200.0, 30.0), "Select", Some(Handler::SelectCharacter),
    ));
    let cs = menu_button(gui, &mut m, smallbutton, csr, Vec2::new(20.0, 20.0), "", Some(Handler::CharacterSmallButton));
    gui.nodes[cs].visible = false;
    m.char_small = Some(cs);
    let cd = menu_button(
        gui, &mut m, button, csr, Vec2::new(300.0, 30.0), "Delete Character", Some(Handler::DeleteCharacter),
    );
    gui.nodes[cd].visible = false;
    m.char_delete = Some(cd);
    m.world_select = Some(menu_button(
        gui, &mut m, button2, wsr, Vec2::new(200.0, 30.0), "Select", Some(Handler::SelectWorld),
    ));
    let ws = menu_button(gui, &mut m, smallbutton, wsr, Vec2::new(20.0, 20.0), "", Some(Handler::WorldSmallButton));
    gui.nodes[ws].visible = false;
    m.world_small = Some(ws);
    let wd = menu_button(
        gui, &mut m, button, wsr, Vec2::new(300.0, 30.0), "Delete World", Some(Handler::DeleteWorld),
    );
    gui.nodes[wd].visible = false;
    m.world_delete = Some(wd);
    // The chat (400x200).
    let (chn, chw) = game_widget(gui, g, GameWidgetClass::Chat);
    gui.set_size(chw, Vec2::new(400.0, 200.0), true);
    m.chat_node = Some(chn);
    m.chat = Some(chw);
    // 0x00465a0a..0x00465a2f: `equippedpreview` and `preview` re-attached to their parent,
    // so they draw over the panels.
    super::item_preview::raise_previews(gui, m.equipped_preview, m.preview);
    // 0x00465a3a: `Engine` 0x006526b0, whose first step is `Node::updateWidgets` 0x00635700
    // from the engine root: pre-order over the nodes without `Node.flags` bit 2, each node's
    // widget gets slot 7 `update`. The port runs it for the Edits only (0x00637e80 finds the
    // Edit's TextShape, `Edit+0x160`: without it the name fields ignore clicks and
    // characters); running every class's slot 7 here moved the buttons and hid the style
    // arrows (cw-ui's Button/Widget updates after the ctor's sizes: not investigated). Its
    // second step (slot 7 and event 0 over the engine's widget list `+0x7c`) and the second
    // `updateWidgets` on 0x00487490's node are not ported either.
    update_widgets(gui, top);
    // Every clone of the ctor takes its template's Transformation and Display along.
    let clones = take_clone_log();
    plx.register_clones(gui, &clones);
    m
}

/// Sets the `texture` attribute of a node's mesh shape (every key and the working value),
/// as 0x00467f10 writes the current key.
pub fn set_shape_texture(gui: &mut Gui, n: NodeId, texture: i32) {
    let Some(sh) = gui.nodes[n].shape.as_ref().and_then(|s| s.as_any()).and_then(|a| a.downcast_ref::<cw_ui::loader::SharedShape>()) else {
        return;
    };
    if let cw_ui::loader::SceneShape::Mesh(m, _) = &mut *sh.borrow_mut() {
        let t = &mut m.source.texture;
        for k in t.keys.iter_mut() {
            *k = texture;
        }
        if t.current == texture && t.keys.iter().all(|k| *k == texture) {
            return;
        }
        t.current = texture;
        t.changed = true;
    }
}

/// A float attribute of a `SmoothMeshShape` the game writes through `0x00467f10` (every key
/// and the working value, as [`set_shape_texture`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeAttr {
    /// `+0x494` `textureBrightness` (the menu buttons' hover, 0x0048ec02: 1.5 / 1).
    TextureBrightness,
    /// `+0x544` `textureSaturation` (the skill icons: 1 usable, 0 greyed).
    TextureSaturation,
}

/// Writes a float attribute of a node's mesh shape (see [`ShapeAttr`]).
pub fn set_shape_attr_f32(gui: &mut Gui, n: NodeId, attr: ShapeAttr, v: f32) {
    let Some(sh) = gui.nodes[n].shape.as_ref().and_then(|s| s.as_any()).and_then(|a| a.downcast_ref::<cw_ui::loader::SharedShape>()) else {
        return;
    };
    if let cw_ui::loader::SceneShape::Mesh(m, _) = &mut *sh.borrow_mut() {
        let a = match attr {
            ShapeAttr::TextureBrightness => &mut m.source.texture_brightness,
            ShapeAttr::TextureSaturation => &mut m.source.texture_saturation,
        };
        if a.current == v && a.keys.iter().all(|k| *k == v) {
            return;
        }
        for k in a.keys.iter_mut() {
            *k = v;
        }
        a.current = v;
        a.changed = true;
    }
}

impl GcMembers {
    /// The handler a signal from `w` runs, if `w` is one of the connected buttons.
    pub fn handler_of(&self, w: WidgetId) -> Option<Handler> {
        self.connections.iter().find(|(x, _)| *x == w).map(|(_, h)| *h)
    }

    /// The members `onResize` 0x00482a40 reads, in cw-ui's struct (same field names; the
    /// renames of [`CW_UI_CORRECTIONS`] are applied there).
    pub fn on_resize_widgets(&self, gui: &Gui) -> GameControllerWidgets {
        let _ = gui;
        GameControllerWidgets {
            speech_node: self.speech_node,
            speech: self.speech,
            questtag: self.questtag,
            smallquesttag: self.smallquesttag,
            cubeworld: self.cubeworld,
            start_menu_node: self.start_menu_node,
            picroma: self.picroma,
            combopoints: self.combopoints,
            wait: self.wait,
            name_error: self.name_error,
            connection_error: self.connection_error,
            character_name: self.character_name,
            create_character: self.create_character,
            char_style_panel: self.char_style_panel,
            options_panel: self.options_panel,
            system_panel: self.system_panel,
            back: self.back,
            multiplayer_worlds: self.multiplayer_worlds,
            server_name: self.server_name,
            connect_to_server: self.connect_to_server,
            connect: self.connect,
            world_name: self.world_name,
            world_seed: self.world_seed,
            create_world: self.create_world,
            character_previews: self.character_previews.clone(),
            quick_slots: self.quick_slots.clone(),
            menu_buttons: self.menu_buttons.clone(),
            inventory_panel: self.inventory_panel,
            landname: self.landname,
            chat: self.chat,
            voxel_panel: self.voxel_panel,
            objective_node: self.objective_node,
            statistics_node: self.statistics_node,
            equipment_boxes: self.equipment_boxes.clone(),
            character: self.character,
            crafting_panel: self.crafting_panel,
            craft_preview_panel: self.craft_preview_panel,
            skills_panel: self.skills_panel,
            shop_panel: self.shop_panel,
            enchant_panel: self.enchant_panel,
            adaption_panel: self.adaption_panel,
            help: self.help,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn construct_without_plx_builds_the_menu_tree() {
        let mut gui = Gui::new();
        let m = construct(&mut gui, &mut NoPlx);
        // The five menu screens start hidden, the start screen and GUI root visible.
        for r in [m.char_create_root, m.world_create_root, m.char_select_root, m.server_root, m.world_select_root] {
            assert!(!gui.nodes[r.unwrap()].visible);
        }
        assert!(gui.nodes[m.start_root.unwrap()].visible);
        assert!(gui.nodes[m.gui_root.unwrap()].visible);
        // Nine GameController connections from the ctor plus the three hidden small/delete
        // buttons' partners: 12 buttons, each to a distinct handler.
        assert_eq!(m.connections.len(), 12);
        let back = gui.nodes[m.back.unwrap()].widget.unwrap();
        assert_eq!(m.handler_of(back), Some(Handler::Back));
        assert_eq!(Handler::Back.address(), 0x004814f0);
        assert_eq!(m.menu_buttons.len(), 6);
        assert_eq!(m.quick_slots.len(), 6);
        assert_eq!(m.equipment_boxes.len(), 12);
        // onResize runs over the members without panicking.
        let gc = m.on_resize_widgets(&gui);
        cw_ui::widget::game_controller_on_resize(&mut gui, &gc, 1280, 720);
    }

    /// The texts of every TextShape node of `n`'s subtree.
    fn texts(gui: &Gui, n: NodeId) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(t) = &gui.nodes[n].text {
            out.push(String::from_utf16_lossy(t));
        }
        for &c in &gui.nodes[n].children {
            out.extend(texts(gui, c));
        }
        out
    }

    /// With the shipped `.plx` files: the menu buttons carry the ctor's texts (no template
    /// "Button" left), the caption nodes do not take events, and the style and options widgets
    /// have their arrows and buttons. Skipped without `CW_GAME_DIR`.
    #[test]
    fn menu_buttons_captions_and_arrows() {
        let Some(dir) = std::env::var_os("CW_GAME_DIR").map(std::path::PathBuf::from) else {
            eprintln!("CW_GAME_DIR not set; skipped");
            return;
        };
        let mut plx = crate::ui::plx_files::GamePlxLoader::new(&dir);
        let mut gui = Gui::new();
        gui.viewport = glam::IVec2::new(1280, 720);
        let m = construct(&mut gui, &mut plx);
        for (n, want) in [
            (m.create_character, "Create Character"),
            (m.create_world, "Create World"),
            (m.back, "Back"),
            (m.multiplayer_worlds, "Multiplayer Worlds..."),
            (m.connect_to_server, "Connect to server"),
            (m.connect, "Connect"),
            (m.char_select, "Select"),
            (m.char_delete, "Delete Character"),
            (m.world_select, "Select"),
            (m.world_delete, "Delete World"),
            (m.char_small, "x"),
        ] {
            let n = n.unwrap();
            let t = texts(&gui, n);
            assert!(!t.is_empty() && t.iter().all(|s| s == want), "{want}: {t:?}");
            // 0x0046eb90(0) on the template's children: only the button itself takes events.
            for &c in &gui.nodes[n].children {
                assert!(gui.nodes[c].flags & cw_ui::widget::node_flags::NO_EVENTS != 0);
            }
        }
        // "Select" is a `button2` clone, the small button a `smallbutton` clone.
        assert_eq!(gui.nodes[m.char_select.unwrap()].name, "button2");
        assert_eq!(gui.nodes[m.char_small.unwrap()].name, "smallbutton");
        // Ten style arrows plus the palette, 22 option arrows, Apply / OK / Cancel.
        assert_eq!(m.style_arrows.len(), 11);
        assert_eq!(m.option_arrows.len(), 22);
        assert_eq!(m.option_buttons.len(), 3);
        let ok = gui.widgets[m.option_buttons[1].0].node;
        assert!(texts(&gui, ok).iter().all(|s| s == "OK"));
        // The arrows are visible children of the widgets' nodes, laid out on their rows.
        let sw = m.char_style.unwrap();
        let (a, _) = m.style_arrows[0];
        assert_eq!(gui.nodes[gui.widgets[a].node].parent, Some(gui.widgets[sw].node));
        assert!(gui.nodes[gui.widgets[a].node].visible);
        assert!((gui.get_position(a) - Vec2::new(100.0, 12.0)).abs().max_element() < 1e-3);
    }

    /// A `LEFT_PRESS` on the palette runs 0x0042b8b0 (hair colour = the hovered colour) and
    /// plays the menu-select sound; on an options button OK applies and hides the panel.
    #[test]
    fn style_and_options_presses_are_routed() {
        use crate::ui::{GameUi, GameView, UiAction};
        use cw_ui::widget::UiEvent;
        let mut game = GameView::default();
        let mut ui = GameUi::new(Gui::new(), &mut NoPlx, &game);
        let (palette, arrow) = *ui.m.style_arrows.last().unwrap();
        assert_eq!(arrow, super::super::character_style::Arrow::PickColor);
        ui.char_style.hover = [1, 2, 3];
        let a = ui.apply(&[UiEvent::Signal { widget: palette, event: event::LEFT_PRESS }], &mut game);
        assert_eq!(ui.char_style.hair, [1, 2, 3]);
        assert_eq!(a, vec![UiAction::PlaySound { id: 0x55, volume: 1.0, pitch: 1.0 }]);
        // A release on it does nothing more.
        assert!(ui.apply(&[UiEvent::Signal { widget: palette, event: event::LEFT_RELEASE }], &mut game).is_empty());
    }
}

