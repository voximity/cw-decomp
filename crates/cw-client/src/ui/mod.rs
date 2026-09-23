//! The client's game UI (Cube.exe): the widget tree the `GameController` constructor
//! 0x00459c40 builds, the per-frame logic of the `cube::*Widget`s (their vtable slot 1) and of
//! the UI half of `GameController::update` 0x00488ee0, and the click/key handlers that turn
//! UI signals into game actions.
//!
//! Tier B. Rendering is not here (cw-render draws); this module decides which nodes are
//! visible and what text and values each widget shows, and emits [`UiAction`]s for the
//! controller. Layout lives in cw-ui (`game_controller_on_resize`, the widgets' `layout`).
//!
//! # Module map
//!
//! | Module | Original |
//! |---|---|
//! | [`members`] | the ctor 0x00459c40 (tree, members, connections) |
//! | [`flow`] | the menu flow: handlers 0x004814f0, 0x004815e0, 0x00481d30, 0x00481fe0, 0x004816f0, 0x004821a0, 0x00482530, 0x00483e70, 0x00484170, 0x00484230; `onMouseDown` 0x0047b600 (start menu, menu bar, right click); the key toggles of `onKeyDown` 0x0047e1b0 (0x00488bd0, 0x00488c00, 0x00488c70, 0x00488d00); the screen rules of `update` 0x00488f..0x00489ca0; `isCursorFree` 0x0047f1d0 |
//! | [`start_menu`] | `StartMenuWidget::update` 0x00583320, `SystemWidget::update` 0x00587710 |
//! | [`character_style`] | `CharacterStyleWidget` ctor 0x00427ce0, update 0x00428e40, layout 0x0042bb00, callbacks 0x0042b810..0x0042bab0, apply 0x0042c080 |
//! | [`options_menu`] | `OptionsWidget` ctor 0x004cf3c0, update 0x004d0230, callbacks 0x004d4430..0x004d4dc0, 0x004d4cb0/0x004d4d20, 0x004d4de0 |
//! | [`chat`] | `ChatWidget` ctor 0x004393b0, update 0x00439730, print 0x0043a500, keys 0x0043a0d0, chars 0x0043a010; the Enter handling and commands of `onKeyDown` 0x0047e1b0 |
//! | [`skills`] | `SkillWidget` point allocation (`onMouseDown` 0x0047b600, 0x00488c70) |
//! | [`skill_panel`] | the skill and specialization buttons of `update` 0x0048da47..0x0048eaef, `SkillWidget::update` 0x004dd810, the skill clicks 0x0047b849 / 0x0047c806, the quick bar's icons and cooldowns 0x0048bd8c..0x0048c100 |
//! | [`item_preview`] | the preview sources 0x0047ae10 / 0x0047b450 / 0x0047b1b0 / 0x0047b340 / 0x0047b550 / 0x0047b3e0 / 0x0047b010, the placement 0x0048ec97..0x0048f17e, `PreviewWidget::update` 0x004d50a0, the Tab selector texts 0x00497b0b.. |
//! | [`hud`] | the HUD bars of `update` 0x00488ee0: experience, life, mana, stamina, cast, charge, mana cubes, pet, party, combo points |
//! | [`target`] | the nameplates (`*lifebar:small` clones), the crosshairs and prompt, the quest tags (`update` 0x0048d054.., 0x00490920..0x0049ad13) |
//! | [`map_overlay`] | `MapOverlayWidget::update` 0x004c9680 (site and cell labels, teleport pick) |
//! | [`speech`] | `SpeechWidget::update` 0x004e5f90 (typing sound, options) |
//! | [`inventory`] | the inventory clicks of `onMouseDown` 0x0047b600 (merge, swap, split, use, learn, equip, sell, buy, buy-back) and the item helpers (`canEquip` 0x0043e420, 0x004c76a0, 0x004c6f20, 0x0042f4a0, 0x0047f9f0, `addItem` 0x0046ebe0) |
//! | [`item_description`] | the item description block 0x004a28c0 (preview, comparison blocks, identification and adaption panels) |
//! | [`gui_models`] | the item models the widgets draw with 0x004758c0 (inventory cells, equipment boxes `SpriteWidget` 0x0051c3d0, preview, panel icons) as `cw_render::passes::GuiModel`s |
//! | [`inventory_widget`] | `InventoryWidget` ctor 0x004c1bb0, update 0x004c2050, scrolling and tabs; the shop data 0x004a2300 |
//! | [`crafting`] | `BlueprintPreviewWidget` update 0x0042f910 / layout 0x004348f0, the recipes 0x0059cff0, the refresh 0x004a14c0, the craft 0x004709c0 |
//! | [`enchant`] | `EnchantWidget` (identification) update 0x0044ea30 / layout 0x00450b90, the trade panel rules 0x004964e7 |
//! | [`adaption`] | `AdaptionWidget` update 0x0040f8f0 / layout 0x00411410 |
//! | [`voxel`] | `VoxelWidget` (weapon customization) update 0x00588500, placement 0x00588250 |
//! | [`previews`] | `CharacterPreviewWidget` 0x00425450, `WorldPreviewWidget` 0x00605ae0, their builders 0x0049d650 / 0x004a23d0 and the carousel of `update` 0x00489d4f..0x0048b2cb |
//! | [`objective`], [`statistics`] | `ObjectiveWidget` (+0x800948), `StatisticsWidget::update` 0x00583be0 (both empty in this build) |
//! | [`names`] | the name generator 0x005a0ed0 |
//! | [`textdb`] | `cube::Speech` 0x004e1970 (`dict_en.xml` of data4.db), the lookup 0x00661830, the markup parser 0x004da850 and renderer 0x004e4350 |
//! | [`tooltip`] | the skill variables 0x00478800, the skill/specialization tooltips 0x004a5710 / 0x004a62c0, the objective joiner 0x00477fa0, the trainer test 0x0047f030 |
//! | [`plx_files`] | `Engine::loadFile` 0x00653770 of the five `.plx` files over cw-ui's loader |
//! | `crate::names`, `crate::prompts` | the World's name helpers (item, creature, site, cell, region names, objective text, station test) and the crosshair prompts, used by [`GameUi::frame`] unless [`FrameInputs`] overrides them |
//!
//! # State split
//!
//! [`GameUi`] owns the [`Gui`] and the per-widget state (the fields of the `cube::*Widget`
//! objects and the GameController's UI bytes). [`GameView`] is the client state the UI reads
//! and edits: the local player (`GC+0x8006d0`), money, inventory, the saved characters and
//! worlds, the options block and the connection flags. [`GameUi::apply`] routes cw-ui's
//! [`UiEvent`]s to the handlers and returns the [`UiAction`]s the controller must perform.

#![allow(dead_code)]

pub mod adaption;
pub mod bubbles;
pub mod character_sheet;
pub mod character_style;
pub mod chat;
pub mod crafting;
pub mod enchant;
pub mod flow;
pub mod hud;
pub mod inventory;
pub mod gui_models;
pub mod item_description;
pub mod inventory_widget;
mod inventory_input;
pub mod item_preview;
pub mod quick_item;
pub mod map_overlay;
pub mod members;
pub mod names;
pub mod objective;
pub mod options_menu;
pub mod plx_files;
pub mod present;
pub mod present_hud;
pub mod present_panels;
pub mod previews;
pub mod skill_panel;
pub mod skills;
pub mod speech;
pub mod start_menu;
pub mod statistics;
pub mod target;
pub mod textdb;
pub mod tooltip;
pub mod voxel;

use cw_net::EntityData;
use cw_ui::widget::{Gui, UiEvent, event};

use std::sync::Arc;

use cw_math::rand::MsvcRand;
use glam::{IVec2, Vec2};

use crate::options::Options;

pub use members::GcMembers;

/// One item stack of the inventory (`count` i32 + `cube::Item`, 0x11c bytes in a tab vector
/// at `creature+0x11dc`). The item is kept as its raw 0x118 bytes (`cw_net::ITEM_SIZE`).
#[derive(Clone, Debug, PartialEq)]
pub struct ItemStack {
    /// +0: the stack count.
    pub count: i32,
    /// +4: the item.
    pub item: Vec<u8>,
}

impl ItemStack {
    /// An empty slot (count 0, zeroed item).
    pub fn empty() -> Self {
        ItemStack { count: 0, item: vec![0; cw_net::entity::ITEM_SIZE] }
    }

    /// `Item+0` the item type.
    pub fn item_type(&self) -> u8 {
        self.item[0]
    }

    /// `Item+1` the sub type.
    pub fn sub_type(&self) -> u8 {
        self.item[1]
    }
}

/// The character style of the local creature: the first 0x14 bytes of the 0x40-byte record at
/// `*(creature+0x1d28)` (ctor 0x0044a7e0), which the creation screen writes (see
/// [`character_style`]). The rest of that record is the learned formulas (`+0x14`,
/// [`GameView::formulas`]), the per-world positions (`+0x1c`, `crate::persist`) and the
/// current world (`+0x24`/`+0x28`, [`GameView::current_world`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Style {
    /// +0: race index 0..7 (Human, Elf, Dwarf, Orc, Goblin, Lizard, Undead, Frogman).
    pub race: i32,
    /// +4: gender (0 male, 1 female).
    pub gender: u8,
    /// +8: face index within the race's range.
    pub face: i32,
    /// +0xc: haircut index within the race's range.
    pub haircut: i32,
    /// +0x10..+0x12: hair colour.
    pub hair_color: [u8; 3],
}

impl Default for Style {
    /// The record's ctor 0x0044a7e0: race 0, gender 0, variants 0, hair (255, 255, 255).
    fn default() -> Self {
        Style { race: 0, gender: 0, face: 0, haircut: 0, hair_color: [0xff; 3] }
    }
}

/// A saved character as the select screen shows it (`Save/characters.db`, vector at
/// `GC+0x800984`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CharacterEntry {
    /// Name (`creature+0x1168`).
    pub name: String,
    /// Level, class, specialization for the preview text.
    pub level: i32,
    /// Class (1 warrior .. 4 rogue).
    pub class: u8,
    /// Specialization.
    pub specialization: u8,
    /// Entity type (`creature+0x64` = entity+0x54: 0/1 elf, 2/3 human, 4/5 goblin, 7/8
    /// lizardman, 9/10 dwarf, 11/12 orc, 13/14 frogman, 15/16 undead), for the preview's
    /// race word (`CharacterPreviewWidget::update` 0x00425450).
    pub entity_type: i32,
}

/// A saved world (`cube::WorldInfo`, 0x28 bytes, vector at `GC+0x8009dc`): name at +8, seed
/// at +0x20.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorldEntry {
    /// +8.
    pub name: String,
    /// +0x20.
    pub seed: i32,
    /// +0x24: explored area (int, shown as `"Explored: " << (float)v << " km²"` by
    /// `WorldPreviewWidget::update` 0x00605ae0).
    pub explored: i32,
}

/// The client state the UI reads and edits, with the GameController field each part mirrors.
#[derive(Clone, Debug)]
pub struct GameView {
    /// `GC+0x8006d0 + 0x10`: the local player's entity data.
    pub player: EntityData,
    /// `creature+0x1d28`: the character style.
    pub style: Style,
    /// `creature+0x1304`: coins.
    pub coins: i32,
    /// `creature+0x1308`: platinum coins.
    pub platinum: i32,
    /// `creature+0x11dc`: the inventory tabs (vector of vectors of stacks).
    pub inventory: Vec<Vec<ItemStack>>,
    /// `creature+0x11e8 / +0x11ec`: the stack held by the cursor (count 0 = none).
    pub held: ItemStack,
    /// `creature+0x300 + k * 0x118`, k = 0..11: the equipment (entity slots 1..12).
    pub equipment: Vec<Vec<u8>>,
    /// `GC+0x800984`: the saved characters.
    pub characters: Vec<CharacterEntry>,
    /// `GC+0x800a0c`: the character the select screen shows (== len: "New character").
    pub selected_character: i32,
    /// `GC+0x8009dc`: the saved worlds.
    pub worlds: Vec<WorldEntry>,
    /// `GC+0x800a10`: the world the select screen shows (== len: "New world").
    pub selected_world: i32,
    /// `GC+0x8009b0`: the world list shows multiplayer worlds.
    pub multiplayer: bool,
    /// `GC+0x170..+0x19c`: the options block.
    pub options: Options,
    /// The display modes WinMain enumerated (passed to the ctor, `OptionsWidget+0x160`).
    pub display_modes: Vec<(i32, i32)>,
    /// `GC+0x800585`: connected to a server.
    pub connected: bool,
    /// `GC+0x8006cc`: the socket is open.
    pub socket_open: bool,
    /// `GC+0x1d4`: the smoothed distance to the nearest unmeshed chunk (the "Please
    /// wait..." test is `< 8`).
    pub load_distance: f32,
    /// `GC+0x800a50 != GC+0x800448`, or zone generation pending (0x0040c520): the world is
    /// not ready.
    pub world_pending: bool,
    /// `GC+0x8006e4`: the map is open.
    pub map_open: bool,
    /// Engine+0xe8: engine time in ms (caret blink, `/ 500` parity).
    pub engine_time_ms: i32,
    /// `GC+0x1a0`: the quit flag.
    pub quit: bool,
    /// The local player's current-world record (`*(player creature+0x1d28)`: seed at +0x24,
    /// name at +0x28; written by `startWorld` 0x0046f620). The world preview draws the
    /// selected character walking on the world whose seed and name match
    /// (`WorldPreviewWidget::update` 0x00605ae0).
    pub current_world: WorldEntry,
    /// The learned formulas (`*(creature+0x1d28) + 0x14`, a `std::list` of raw 0x118-byte
    /// items: a used type-2 item with its type byte replaced by `Item+8` and `Item+8..+0xc`
    /// zeroed; appended by [`inventory::use_or_equip`], read by `0x00444a90` and the recipe
    /// rebuild 0x004a14c0).
    pub formulas: Vec<Vec<u8>>,
    /// `GC+0x800a44`: the quick-item (Tab) selection (`crate::player::LocalPlayer::quick_index`).
    pub quick_index: i32,
    /// The skill bar `GC+0x800814` with each slot's cooldown state (see
    /// [`skill_panel::SkillBarSlot`]).
    pub skill_bar: Vec<skill_panel::SkillBarSlot>,
    /// `0x0047f030`: a class trainer of the player's class is near
    /// ([`tooltip::class_trainer_nearby`]); read by the Learn test 0x004df880 and
    /// `SkillWidget::update` 0x004dd810.
    pub trainer_nearby: bool,
    /// Set by the style widget's apply 0x0042c080, whose tail (the appearance 0x0043f7c0,
    /// then the starter kit 0x004772b0, both drawing the CRT `rand()`) the controller runs
    /// with the main thread's stream ([`character_style::starter_kit`]).
    pub style_applied: bool,
    /// The local player's guard and buffs the character sheet reads (`CharacterWidget`
    /// 0x00434e30, [`character_sheet::SheetState`]).
    pub sheet: character_sheet::SheetState,
}

impl Default for GameView {
    fn default() -> Self {
        GameView {
            player: EntityData::new_creature(),
            style: Style::default(),
            coins: 0,
            platinum: 0,
            inventory: Vec::new(),
            held: ItemStack::empty(),
            equipment: vec![vec![0; cw_net::entity::ITEM_SIZE]; 12],
            characters: Vec::new(),
            selected_character: 0,
            worlds: Vec::new(),
            selected_world: 0,
            multiplayer: false,
            options: Options::default(),
            display_modes: Vec::new(),
            connected: false,
            socket_open: false,
            load_distance: 0.0,
            world_pending: false,
            map_open: false,
            engine_time_ms: 0,
            quit: false,
            current_world: WorldEntry::default(),
            formulas: Vec::new(),
            quick_index: 0,
            skill_bar: Vec::new(),
            trainer_nearby: false,
            style_applied: false,
            sheet: character_sheet::SheetState::default(),
        }
    }
}

/// What the UI asks the controller to do.
#[derive(Clone, Debug, PartialEq)]
pub enum UiAction {
    /// `GC+0x1a0 = 1` (start menu "Exit", system "Exit Game").
    Quit,
    /// `GameController::startWorld(int seed, std::string name)` `Cube.exe 0x0046f620` (world
    /// "Select" 0x00484170, "Create World" 0x00482530; the system menu's "Start Menu" calls
    /// it with `timeGetTime()` and "" — see [`UiAction::StartWorldTimeSeed`]). It does not
    /// generate or load anything itself; it only retargets the world and clears it:
    ///
    /// 1. Locks the world (`World(GC+0x2e4)+0x8000c0` via 0x00601cb0) and the GC world
    ///    critical section `GC+0x8005d0`.
    /// 2. Writes `(seed, name)` into the local player's current-world record
    ///    (`*(player+0x1d28)`: +0x24 seed, +0x28 name; [`GameView::current_world`]).
    /// 3. `GC+0x800a50 = seed`, `GC+0x800a54 = name` (the requested world).
    /// 4. Collects every creature of the world's creature map (`World+4`, `[GC+0x2e8]`) other
    ///    than the local player (`GC+0x8006d0`), then deletes each (vtable slot 0 with 1) and
    ///    erases it from the map (0x0043ede0). Unlocks both.
    ///
    /// The switch itself happens in the zone thread 0x0046a8a0 (every pass, 20 ms sleep):
    /// while `GC+0x800a50 != World seed (World+0x800164 = GC+0x800448)` or the name differs
    /// from the world's (`GC+0x378`), it unloads every loaded zone/sector (0x005a4890,
    /// 0x005a4800, 0x005a4780 over the 1024x1024 sector table), then under the world lock
    /// runs 0x0059c480, `World::load(seed, name)` 0x005a52e0 and 0x005fbc90. When the name
    /// is not empty it searches the saved worlds (`GC+0x8009dc`) for the same name and seed
    /// and sets `GC+0x800a10` to it; if none matches (a server's world), it appends a new
    /// `cube::WorldInfo {seed, name}`, selects it, saves it (0x004878a0(sel, info, 0), after
    /// writing the count+1 record 0x004499c0) and rebuilds the world previews (0x004a23d0).
    /// Finally it places the player: the position saved for `(seed, name)` in the map at
    /// `*(player+0x1d28)+0x1c` (0x0044b880 find / 0x0044b460), or else the world's spawn
    /// point (`GC+0x8003d4` floats × 65536, `__ftol2`); copies it to the render position
    /// (`creature+0x1350..`) and zeroes the velocity (`creature+0x34..+0x3f`). The "Please
    /// wait..." label ([`GameView::world_pending`]) covers the switch. No in-process server is
    /// started here (`GC+0x8006c8` is created elsewhere; see client-classes.md).
    StartWorld { seed: i32, name: String },
    /// The system menu's "Start Menu" (`onMouseUp` 0x0047e12f): `startWorld(timeGetTime(),
    /// "")` — a throw-away title-screen world seeded by the clock.
    StartWorldTimeSeed,
    /// Reload every saved character from `Save/characters.db` into its creature
    /// (`0x004806c0(i, characters[i])` for each `i`, system menu "Start Menu" 0x0047e0d0).
    ReloadCharacters,
    /// The start-screen camera of the system menu's "Start Menu" (0x0047e145):
    /// `GC+0x1bc = 0`, `GC+0x1c0 = 0` (distance), pitch `GC+0x1b0 = 120`, yaw `GC+0x1b8 = 0`.
    StartMenuCamera,
    /// `connect(address)` 0x0046fc50 ("Connect", `/connect`).
    Connect { address: String },
    /// `disconnect` 0x004719f0 (`/disconnect`).
    Disconnect,
    /// The chat line of `onKeyDown` 0x0047e1b0: queued for the send thread when the socket is
    /// open (0x004865b0), else echoed locally with the player's id (0x004861f0).
    SendChat { text: String, local_echo: bool },
    /// `/name`: the player's name (`creature+0x1168`).
    SetName { name: String },
    /// `/namepet`: the pet item's name (at most 15 characters).
    SetPetName { name: String },
    /// A chat line printed by the UI itself (`0x0043ab30` with an RGBA colour).
    Print { text: String, color: [f32; 4] },
    /// `GameController::applyOptions` 0x0046f390: copy the block, save `options.cfg`, set the
    /// music volume.
    ApplyOptions(Options),
    /// `playSound(id, position, volume, pitch)` 0x00484350 / `playSoundAtListener` 0x00484320.
    PlaySound { id: u32, volume: f32, pitch: f32 },
    /// The camera reset of the select/create handlers: `GC+0x1c0 = GC+0x1bc = 5`, pitch 90,
    /// roll 0, yaw `yaw`.
    ResetCamera { yaw: f32 },
    /// The menu-screen camera orbit of `update`: distance 10, pitch 100, yaw += dt * 0.005.
    OrbitCamera,
    /// Save character `index` (0x00487520) after "Create Character".
    SaveCharacter { index: i32 },
    /// Load character `index` into the player (0x004806c0).
    LoadCharacter { index: i32 },
    /// Reset the player for a new character (0x00446330 and the field resets of 0x00483e70).
    NewCharacter,
    /// Delete character `index` and rewrite `Save/characters.db` (0x004816f0).
    DeleteCharacter { index: i32 },
    /// Save world `index` (0x004878a0).
    SaveWorld { index: i32 },
    /// Delete world `index` and its `Save\world_<name>`/`Save\map_<name>` files (0x00481d30).
    DeleteWorld { index: i32, name: String },
    /// Rebuild the character previews (0x0049d650) and the world previews (0x004a23d0):
    /// call [`previews::rebuild_character_previews`] and [`previews::rebuild_world_previews`].
    /// Each detaches the screen's Select / 20x20 / Delete buttons, destroys every child of the
    /// select screen (0x00632870), clears the preview vector (and, for worlds, the two
    /// node<->index maps `GC+0x8009cc` / `GC+0x8009d4`), then for each saved entry plus one
    /// "New" entry clones `blackwidget` into the screen, sizes it 600x220, attaches a new
    /// `CharacterPreviewWidget` / `WorldPreviewWidget` (+0x160 = the creature / `WorldInfo`,
    /// null for "New") and places it at (300, 50 + 230·k); worlds whose name starts with
    /// `online_` are only listed when `GC+0x8009b0` (multiplayer) is set and vice versa.
    /// Finally the three buttons are re-attached to the screen (drawn on top).
    RefreshPreviews,
    /// Learn the skill allocation shown in the skill panel (cost paid by the UI).
    LearnSkills,
    /// `GameController::useItem(stack)` 0x004a2780 on bag cell `(tab, index)` under the world
    /// lock (`GC+0x2e4`, 0x00601cb0 / 0x00601e90): [`crate::interact::use_item`] (a type-1
    /// consumable right/middle-clicked in the bag, `onMouseDown` 0x0047d717).
    UseItem { tab: i32, index: i32 },
    /// A right/middle click made `item` the target of another panel (`onMouseDown`
    /// 0x0047b600): the adaption widget (`+0x800904`: `+0x160` item pointer, or `+0x164/+0x168`
    /// bag cell), the enchant widget (`+0x8008fc`: `+0x160/+0x164` bag cell) or the voxel
    /// widget (`+0x8008f4`: `+0x160` item pointer, then 0x0058ce20).
    SetItemTarget { panel: inventory::TargetPanel, item: inventory::ItemRef },
    /// `0x004a2300`: rebuild the shop's pages (`GC+0x800c0c`) from the vendor or the
    /// buy-back list: [`inventory_widget::ShopData::refresh`].
    RefreshShop,
    /// `0x004a14c0`: rebuild the crafting widget's recipe tabs from [`GameView::formulas`]
    /// (run after every right/middle click of `onMouseDown`, 0x0047dd88).
    RebuildRecipes,
    /// `0x00480fb0(item, player)`: the "You receive <item>" chat line for an item the local
    /// player got (after a craft 0x004709c0 or an identification). The item is already in the
    /// bag; the bag widget (`GC+0x800954`) refreshes (0x004c6350 / 0x004c64c0).
    ItemReceived { item: Vec<u8> },
    /// Items pushed onto the pickup queue `GC+0x1000e68` (0x0044d460) with the pickup timer
    /// `GC+0x1000e70` reset to 0 (the unreachable disenchant mode of the identification
    /// panel, [`enchant::identify_click`]); `crate::player::LocalPlayer::pending_pickups`.
    QueuePickups { items: Vec<Vec<u8>> },
}

/// The GameController's UI bytes and the per-widget state.
#[derive(Debug, Default)]
pub struct GameUi {
    /// The widget tree.
    pub gui: Gui,
    /// The members of the tree.
    pub m: GcMembers,
    /// `GC+0x8008f0`: the equipment slots are live; written every frame as "the inventory
    /// panel is visible" (0x0048c6c1), cleared by right click on nothing.
    pub flag_8008f0: bool,
    /// `GC+0x8008f1`: the skills panel mode (X).
    pub skills_open: bool,
    /// `GC+0x8008f2`: the system menu bar (Esc).
    pub system_menu: bool,
    /// `GC+0x8007b4`: toggled by V: nameplates for every creature, with the rare names
    /// ([`target::PlateInputs::show_all`]).
    pub flag_8007b4: bool,
    /// `GC+0x800a40`: toggled by Tab: the quick-item menu, which shows the `selector`
    /// (+0x8008e0, 0x004927c7).
    pub flag_800a40: bool,
    /// `StartMenuWidget+0x160` / `SystemWidget+0x160`: the hovered item.
    pub start_menu: start_menu::MenuState,
    /// `SystemWidget+0x160`.
    pub system_menu_state: start_menu::MenuState,
    /// `CharacterStyleWidget` fields.
    pub char_style: character_style::CharacterStyle,
    /// `OptionsWidget` fields.
    pub options: options_menu::OptionsMenu,
    /// `ChatWidget` fields.
    pub chat: chat::ChatWidget,
    /// `SkillWidget` fields.
    pub skills: skills::SkillWidget,
    /// The dialog `SpeechWidget` (+0x800938) fields.
    pub speech: speech::SpeechWidget,
    /// The four speech bubbles' `SpeechWidget`s (`GC+0x80092c[4]`, [`bubbles`]), parallel to
    /// `hud::HudNodes::bubbles`.
    pub bubbles: Vec<speech::SpeechWidget>,
    /// The HUD content of the last frame.
    pub hud: hud::HudFrame,
    /// The HUD bars' state (the hit counter of the last frame, 0x0076b101).
    pub hud_state: hud::HudState,
    /// The small quest tag's state (`smallquesttag`, the last cell shown).
    pub quest_tags: target::QuestTagState,
    /// The character and world select screens (`CharacterPreviewWidget`s,
    /// `WorldPreviewWidget`s, the carousel velocities `GC+0x800a04` / `+0x800a08`).
    pub previews: previews::Previews,
    /// The three `InventoryWidget`s (bag +0x800954, crafting +0x800958, shop +0x80095c) and
    /// the shop data (`GC+0x800c0c`, buy-back `GC+0x800d3c`).
    pub inv: inventory_widget::InventoryUi,
    /// The crafting state (`GC+0x800adc` recipe tabs, `GC+0x1000e78` timer) and the
    /// `BlueprintPreviewWidget` +0x800964.
    pub crafting: crafting::CraftingState,
    /// `EnchantWidget` +0x8008fc (identification).
    pub enchant: enchant::EnchantWidget,
    /// `AdaptionWidget` +0x800904.
    pub adaption: adaption::AdaptionWidget,
    /// `VoxelWidget` +0x8008f4 (weapon customization).
    pub voxel: voxel::VoxelWidget,
    /// `ObjectiveWidget` +0x800948.
    pub objective: objective::ObjectiveWidget,
    /// `StatisticsWidget` +0x800950.
    pub statistics: statistics::StatisticsWidget,
    /// The client World's `cube::Speech` (`World+0x30` = `GC+0x314`, ctor 0x004e1970 over
    /// `data4.db`'s `dict_en.xml`): the dictionary behind every name helper of
    /// [`crate::names`]. `None` without the game folder (every dictionary text then reads
    /// `""`, as the original's lookups of missing keys do).
    pub text: Option<Arc<textdb::TextDb>>,
    /// The `PreviewWidget` (`GC+0x8008e8`) fields.
    pub preview: item_preview::PreviewWidget,
    /// The font measure every text layout of the UI uses (`resource1.dat`).
    pub fonts: present_hud::TextMeasure,
}

/// The crosshair inputs of [`FrameInputs`] (0x004968d0..).
pub struct CrosshairInputs<'a> {
    /// `GC+0x14` (LShift, the zoom crosshair).
    pub shift: bool,
    /// An override of the interaction prompt; `None` computes it with
    /// [`crate::prompts::interaction_prompt`] (0x00496922..0x00496f37) from the fields below.
    pub prompt: Option<&'a str>,
    /// The local player's creature id (`creature+8`).
    pub player_id: i64,
    /// The aimed creature `GC+0x800a70` (`World::findEntity` 0x0042f000), when it exists.
    pub aimed_creature: Option<&'a EntityData>,
    /// The aimed static `GC+0x800a84` (0x005a0910), when it exists.
    pub aimed_static: Option<&'a cw_world::zone::Static>,
}

/// The quest-tag inputs of [`FrameInputs`] (0x0048d054..0x0048d51c, 0x00490a79).
pub struct QuestInputs<'a> {
    /// The World cell under the player.
    pub cell: Option<&'a target::QuestCell>,
    /// Its `Cell::falloff` at the player's fixed x, y.
    pub falloff: f32,
    /// The player's fixed position.
    pub player_pos: [i64; 3],
    /// An override of the objective text; `None` uses [`crate::names::objective_text`]
    /// (0x00477fa0) over [`GameUi::text`].
    pub objective: Option<&'a dyn Fn(&target::QuestCell) -> String>,
    /// `Node::isAnimating` 0x006364f0 on `questtag` (+0x800868): the caller evaluates
    /// [`cw_ui::loader::PlxScene::is_animating`] on the `quest-tag.plx` scene.
    pub questtag_animating: bool,
}

/// The nameplate inputs of [`FrameInputs`] (0x00490920.., 0x0049a..).
pub struct PlateSet<'a> {
    /// The camera and world queries.
    pub inputs: target::PlateInputs<'a>,
    /// [`target::plate_candidates`] over the world's creatures.
    pub candidates: Vec<target::PlateCandidate>,
    /// The creature of a candidate id.
    pub creature: &'a dyn Fn(i64) -> Option<target::PlateCreature<'a>>,
}

/// The customization panel's world inputs (see [`voxel::VoxelInputs`]).
pub struct VoxelWorld<'a> {
    /// The weapon's model.
    pub model: Option<&'a dyn voxel::VoxelModel>,
    /// Its model-cache index.
    pub model_index: Option<u32>,
    /// The cursor ray in model voxel space.
    pub ray: Option<([f32; 3], [f32; 3])>,
}

/// What one [`GameUi::frame`] reads besides the UI state and [`GameView`]: the frame time,
/// the client size, the CRT `rand()` stream, and the world queries of the HUD, nameplates,
/// quest tags, map and customization (each `None` when the controller has no world yet;
/// that part then produces nothing).
pub struct FrameInputs<'a> {
    /// `creature+0x1194` (stamina).
    pub stamina: f32,
    /// `update`'s dt in ms.
    pub dt_ms: i32,
    /// `GC+0x11c`, `GC+0x120`.
    pub client: IVec2,
    /// `Engine+0xe8`: engine time in ms.
    pub time_ms: i32,
    /// `GC+4`: the left mouse button (DirectInput byte) is held.
    pub left_held: bool,
    /// The world's name (`World+0x94`; the objective node shows when it is not empty).
    pub world_name: &'a str,
    /// The CRT `rand()` stream of the main thread (the craft bonus draws).
    pub rng: &'a mut MsvcRand,
    /// The client world (`GC+0x2e4`), for the station test and the map's region names;
    /// `None` without one.
    pub world: Option<&'a cw_world::World>,
    /// An override of `World::itemName` 0x00598a50; `None` uses [`crate::names::item_name`]
    /// over [`GameUi::text`].
    pub item_name: Option<&'a dyn Fn(&[u8]) -> String>,
    /// An override of the station test; `None` uses [`crate::names::station_ok`] on
    /// [`FrameInputs::world`] and the local player (every station counts as present without
    /// a world).
    pub station_ok: Option<&'a dyn Fn(crafting::Station) -> bool>,
    /// The HUD bars' world inputs ([`hud::hud_bars`]).
    pub hud: Option<hud::HudInputs<'a>>,
    /// The crosshair ([`target::crosshair`]).
    pub crosshair: Option<CrosshairInputs<'a>>,
    /// The quest tags ([`target::small_quest_tag`]).
    pub quest: Option<QuestInputs<'a>>,
    /// The nameplates ([`target::nameplates`]).
    pub plates: Option<PlateSet<'a>>,
    /// The map overlay ([`map_overlay::map_overlay`]), with the map open.
    pub map: Option<map_overlay::MapOverlayInputs<'a>>,
    /// The customization panel's model and ray.
    pub voxel: Option<VoxelWorld<'a>>,
    /// The aimed ground item (`GC+0x800a78` through 0x0059fb90), the preview's default item.
    pub ground_item: Option<&'a [u8]>,
}

impl<'a> FrameInputs<'a> {
    /// Inputs without any world part (menus, tests).
    pub fn basic(rng: &'a mut MsvcRand, stamina: f32, dt_ms: i32, client: IVec2) -> Self {
        FrameInputs {
            stamina,
            dt_ms,
            client,
            time_ms: 0,
            left_held: false,
            world_name: "",
            rng,
            world: None,
            item_name: None,
            station_ok: None,
            hud: None,
            crosshair: None,
            quest: None,
            plates: None,
            map: None,
            voxel: None,
            ground_item: None,
        }
    }
}

impl GameUi {
    /// The ctor 0x00459c40 over `gui` with `plx` loading the `.plx` files.
    pub fn new(mut gui: Gui, plx: &mut dyn members::PlxLoader, game: &GameView) -> Self {
        let m = members::construct(&mut gui, plx);
        let size_of = |gui: &Gui, n: Option<cw_ui::widget::NodeId>| {
            n.and_then(|n| gui.nodes[n].widget).map(|w| Vec2::new(gui.width(w), gui.height(w)))
        };
        // The three InventoryWidget ctor calls (0x004c1bb0) with the cell templates.
        let inv = inventory_widget::InventoryUi::new(size_of(&gui, m.itembox), size_of(&gui, m.wideitembox));
        // The World ctor 0x0058eb00 builds its `cube::Speech` (0x004e1970) from `data4.db`.
        let text = plx
            .game_dir()
            .and_then(|d| cw_formats::AssetDb::open(d.join("data4.db")).ok())
            .and_then(|db| textdb::TextDb::load(&db).ok())
            .map(Arc::new);
        let fonts = present_hud::TextMeasure::new(plx.game_dir().unwrap_or_else(crate::controller::default_game_dir));
        let mut ui = GameUi { gui, m, inv, text, fonts, ..Default::default() };
        // 0x004d4de0: the options widget copies the block and finds the current mode.
        ui.options.load(&game.options, &game.display_modes);
        ui
    }

    /// Routes one frame's cw-ui events: a left release on a connected button runs its
    /// GameController handler (`MemberFunctionConnection<GameController>::invoke`
    /// 0x0046f480). Other events are the widgets' own business.
    pub fn apply(&mut self, events: &[UiEvent], game: &mut GameView) -> Vec<UiAction> {
        let mut out = Vec::new();
        for e in events {
            if let UiEvent::Signal { widget, event: ev } = *e {
                if ev == event::LEFT_PRESS {
                    self.on_left_press(widget, game, &mut out);
                    continue;
                }
                if ev != event::LEFT_RELEASE {
                    continue;
                }
                if let Some(h) = self.m.handler_of(widget) {
                    flow::run_handler(self, h, game, &mut out);
                }
            }
        }
        out
    }

    /// The `LEFT_PRESS` (event 2) connections of the style and options widgets
    /// (`MemberFunctionConnection<CharacterStyleWidget / OptionsWidget>::invoke` 0x004c2040):
    /// the style arrows and palette (0x0042b810..0x0042bab0, 0x0042b8b0: the callback, then
    /// the menu-select sound 0x55), the options arrows (0x004d4430..0x004d4dc0, 0x004d4cb0 /
    /// 0x004d4d20) and Apply / OK / Cancel (0x004d4470 / 0x004d4650 / 0x004d4510; OK and
    /// Cancel hide the panel, the options widget's parent node, 0x0062b400).
    fn on_left_press(&mut self, widget: cw_ui::widget::WidgetId, game: &mut GameView, out: &mut Vec<UiAction>) {
        // The InventoryWidgets' tab / up / down buttons (0x004c5a60, 0x004c60f0, 0x004c5a00).
        if self.inventory_left_press(widget, game, out) {
            return;
        }
        if let Some(&(_, arrow)) = self.m.style_arrows.iter().find(|(w, _)| *w == widget) {
            out.push(self.char_style.press(arrow, game));
            return;
        }
        if let Some(&(_, row, right)) = self.m.option_arrows.iter().find(|(w, _, _)| *w == widget) {
            self.options.step(row, right);
            return;
        }
        if let Some(&(_, b)) = self.m.option_buttons.iter().find(|(w, _)| *w == widget) {
            let (actions, hide) = self.options.press(b);
            out.extend(actions);
            if hide {
                let p = self.m.options_panel;
                self.set_visible(p, false);
            }
        }
    }

    /// Visibility of a member node (`Node+0x3c` Display, attribute `+0x94[+0x68]`); a
    /// missing node reads as hidden.
    pub fn visible(&self, n: Option<cw_ui::widget::NodeId>) -> bool {
        n.is_some_and(|n| self.gui.nodes[n].visible)
    }

    /// Writes a member node's visibility (missing nodes are ignored).
    pub fn set_visible(&mut self, n: Option<cw_ui::widget::NodeId>, v: bool) {
        if let Some(n) = n {
            self.gui.nodes[n].visible = v;
        }
    }

    /// The engine cursor in `w`'s local space (0x006294d0).
    pub fn local_cursor(&self, w: Option<cw_ui::widget::WidgetId>) -> glam::Vec2 {
        match w {
            Some(w) => self.gui.widget_world(w).inverse().transform_point2(self.gui.cursor),
            None => self.gui.cursor,
        }
    }

    /// `Button::isHovered` 0x006294c0: the engine's hovered node (`Gui::hovered`) is in
    /// the widget's subtree.
    pub fn is_hovered(&self, w: cw_ui::widget::WidgetId) -> bool {
        let n = self.gui.widgets[w].node;
        self.gui.hovered.is_some_and(|h| self.gui.is_ancestor_or_self(n, Some(h)))
    }

    fn widget_size(&self, w: Option<cw_ui::widget::WidgetId>) -> Vec2 {
        w.map_or(Vec2::ZERO, |w| Vec2::new(self.gui.width(w), self.gui.height(w)))
    }

    fn widget_origin(&self, w: Option<cw_ui::widget::WidgetId>) -> Vec2 {
        w.map_or(Vec2::ZERO, |w| self.gui.widget_world(w).transform_point2(Vec2::ZERO))
    }

    /// A part node's screen rectangle (origin, size): its widget's, or its translation and
    /// shape bounds when it has no widget.
    fn node_rect(&self, n: Option<cw_ui::widget::NodeId>) -> (Vec2, Vec2) {
        let Some(n) = n else { return (Vec2::ZERO, Vec2::ZERO) };
        match self.gui.nodes[n].widget {
            Some(w) => (self.widget_origin(Some(w)), self.widget_size(Some(w))),
            None => {
                let (lo, hi) = self.gui.nodes[n].shape.as_ref().map_or((Vec2::ZERO, Vec2::ZERO), |s| s.bounds());
                (self.gui.node_world(n).transform_point2(lo), hi - lo)
            }
        }
    }

    /// One frame of UI logic, in `update` 0x00488ee0 order (Tier B):
    ///
    /// | Step | Original |
    /// |---|---|
    /// | screen rules | 0x00488f..0x00489ca0 ([`flow::frame_rules`]) |
    /// | select screens | 0x00489d4f..0x0048b2cb ([`previews::frame`]) |
    /// | start / system menus | 0x00583320 / 0x00587710 (their widgets' slot 1) |
    /// | style palette, options | 0x00428e40, 0x004d0230 |
    /// | quest tags | 0x0048d054..0x0048d51c, 0x00490a79 ([`target`]) |
    /// | crafting rules and timer | 0x0048b806.., `GC+0x1000e78` ([`crafting`]) |
    /// | trade panels | 0x004964e7 ([`enchant::trade_panel_rules`]) |
    /// | HUD bars, crosshair, nameplates | 0x00490920..0x0049ad13 ([`hud`], [`target`]) |
    /// | objective node | 0x004908ae / 0x0049845f ([`objective`]) |
    /// | the widgets' slot 1 (engine widget pass) | inventory 0x004c2050, blueprint 0x0042f910, enchant 0x0044ea30, adaption 0x0040f8f0, voxel 0x00588500, statistics 0x00583be0 |
    /// | map overlay | 0x004c9680 ([`map_overlay`]) |
    pub fn frame(&mut self, game: &mut GameView, inp: &mut FrameInputs) -> FrameOutput {
        let mut out = FrameOutput::default();
        // The name helpers (crate::names) unless the caller overrides them.
        let text = self.text.clone();
        let world = inp.world;
        let default_item_name = {
            let t = text.clone();
            move |it: &[u8]| crate::names::item_name(t.as_deref(), it)
        };
        let item_name: &dyn Fn(&[u8]) -> String = match inp.item_name {
            Some(f) => f,
            None => &default_item_name,
        };
        let default_station = {
            let player = game.player.clone();
            move |st: crafting::Station| world.is_none_or(|w| crate::names::station_ok(w, &player, st))
        };
        let station_ok: &dyn Fn(crafting::Station) -> bool = match inp.station_ok {
            Some(f) => f,
            None => &default_station,
        };
        let default_objective = {
            let t = text.clone();
            move |c: &target::QuestCell| crate::names::objective_text(t.as_deref(), &c.objective_cell())
        };
        flow::frame_rules(self, game, &mut out.actions);
        out.previews = previews::frame(&mut self.gui, &self.m, &mut self.previews, game, inp.dt_ms, inp.client);
        if self.visible(self.m.start_menu_node) {
            let w = self.m.start_menu;
            let width = w.map_or(0.0, |w| self.gui.width(w));
            let c = self.local_cursor(w);
            out.start_menu = start_menu::start_menu_update(&mut self.start_menu, c, width);
        }
        if self.visible(self.m.system_panel) {
            let w = self.m.system;
            let width = w.map_or(0.0, |w| self.gui.width(w));
            let c = self.local_cursor(w);
            out.system_menu = start_menu::system_menu_update(&mut self.system_menu_state, c, width);
        }
        if self.visible(self.m.char_create_root) {
            let c = self.local_cursor(self.m.char_style);
            self.char_style.palette_hover(c, game);
            out.style_rows = self.char_style.rows();
        }
        if self.visible(self.m.options_panel) {
            out.option_values = Some(self.options.values());
        }
        // Quest tags.
        if let Some(q) = &inp.quest {
            let objective: &dyn Fn(&target::QuestCell) -> String = match q.objective {
                Some(f) => f,
                None => &default_objective,
            };
            out.quest_tag = Some(target::small_quest_tag(&mut self.quest_tags, q.cell, q.falloff, q.player_pos, objective));
            self.set_visible(self.m.questtag, target::questtag_visible(q.questtag_animating));
        }
        // Crafting: the preview panel rule, the timer and its bar.
        crafting::preview_visibility_rule(&mut self.gui, &self.m);
        out.actions.extend(crafting::update_timer(
            &self.gui, &self.m, &mut self.crafting, game, inp.rng, inp.dt_ms, inp.left_held,
        ));
        if let Some(bar) = self.m.craft_bar.and_then(|n| self.gui.nodes[n].widget) {
            out.craft_bar_fill = Some(crafting::bar_fill(self.crafting.timer, self.gui.width(bar)));
        }
        enchant::trade_panel_rules(&mut self.gui, &self.m);
        // HUD.
        self.hud = hud::hud_frame(&game.player, inp.stamina);
        if let Some(h) = &inp.hud {
            out.hud_bars = Some(hud::hud_bars(&mut self.hud_state, h));
        }
        if let Some(c) = &inp.crosshair {
            let free = flow::is_cursor_free(self, game);
            // 0x00496922..0x00496f37: the interaction prompt (after "[R] Revive").
            let prompt = c
                .prompt
                .or_else(|| crate::prompts::interaction_prompt(&game.player, c.player_id, c.aimed_creature, free, c.aimed_static));
            out.crosshair = Some(target::crosshair(&game.player, free, c.shift, inp.client.x, inp.client.y, prompt));
        }
        if let Some(p) = &inp.plates {
            out.nameplates = target::nameplates(&p.inputs, &p.candidates, p.creature, text.as_deref());
        }
        let objective = objective::frame(&mut self.objective, inp.world_name);
        self.set_visible(self.m.objective_node, objective.visible);
        out.objective = objective;
        out.statistics = statistics::frame(&self.statistics);
        // Voxel cursor caption (replaces the held-count caption).
        out.cursor_caption = match voxel::cursor_caption(&self.gui, &self.m, &self.voxel, item_name) {
            Some(s) => s,
            None => inventory::cursor_caption(&game.held),
        };
        // The widgets' slot 1, for the visible panels.
        let cursor = self.gui.cursor;
        let inv_frame = |ui: &mut GameUi, which: u8, panel, w, game: &GameView, item_name: &dyn Fn(&[u8]) -> String| {
            if !ui.visible(panel) {
                return None;
            }
            let size = ui.widget_size(w);
            let local_size = w.map_or(Vec2::ZERO, |w| ui.gui.local_size(w));
            let origin = ui.widget_origin(w);
            let crafting_tabs = ui.crafting.tabs.clone();
            let (pages, coins, platinum): (&[Vec<ItemStack>], i32, i32) = match which {
                0 => (&game.inventory, game.coins, game.platinum),
                2 => (&crafting_tabs, 0, 0),
                _ => (&ui.inv.shop_data.pages, ui.inv.shop_data.coins, ui.inv.shop_data.platinum),
            };
            let pages = pages.to_vec();
            // 0x004c5620 / 0x004c2050: `Button::isHovered` 0x006294c0 of the part nodes and
            // of each tab.
            let parts = match which {
                0 => &ui.m.bag_parts,
                2 => &ui.m.crafting_parts,
                _ => &ui.m.shop_parts,
            };
            let hovered = |n: Option<cw_ui::widget::NodeId>, name: &str| {
                n.and_then(|n| members::find_node(&ui.gui, n, name))
                    .and_then(|b| ui.gui.nodes[b].widget)
                    .is_some_and(|w| ui.is_hovered(w))
            };
            let (up_hovered, down_hovered, scroll_hovered) =
                (hovered(parts.up, "upbutton"), hovered(parts.down, "downbutton"), hovered(parts.scroll, "scrollbutton"));
            let tab_hovered: Vec<bool> =
                parts.tabs.iter().map(|&t| ui.gui.nodes[t].widget.is_some_and(|w| ui.is_hovered(w))).collect();
            let fi = inventory_widget::FrameInput {
                pages: Some(&pages),
                coins,
                platinum,
                size,
                local_size,
                origin,
                cursor,
                up_hovered,
                down_hovered,
                scroll_hovered,
                tab_hovered: &tab_hovered,
                held: &game.held,
                item_name,
            };
            let wdg = match which {
                0 => &mut ui.inv.bag,
                2 => &mut ui.inv.crafting,
                _ => &mut ui.inv.shop,
            };
            Some(wdg.frame(&fi))
        };
        out.bag = inv_frame(self, 0, self.m.inventory_panel, self.m.inventory, game, item_name);
        out.crafting = inv_frame(self, 2, self.m.crafting_panel, self.m.crafting, game, item_name);
        out.shop = inv_frame(self, 3, self.m.shop_panel, self.m.shop, game, item_name);
        if self.visible(self.m.craft_preview_panel) {
            let w = self.m.craft_preview;
            let button = self.m.craft_button.and_then(|n| self.gui.nodes[n].widget);
            let pi = crafting::PreviewInputs {
                game,
                origin: self.widget_origin(w),
                size: self.widget_size(w),
                cursor_local: self.local_cursor(w),
                cursor_screen: cursor,
                dt_ms: inp.dt_ms,
                station_ok,
                item_name,
                has_button: self.m.craft_button.is_some(),
                button_hovered: button.is_some_and(|b| self.is_hovered(b)),
                has_bar: self.m.craft_bar.is_some(),
                timer: self.crafting.timer,
            };
            out.craft_preview = Some(crafting::preview_frame(&mut self.crafting.preview, &pi));
        }
        if self.visible(self.m.enchant_panel) {
            let w = self.m.enchant;
            let rect = self.node_rect(self.m.enchant_itemframe);
            out.enchant = Some(enchant::frame(game, &self.enchant, self.local_cursor(w), self.widget_size(w), rect));
        }
        if self.visible(self.m.adaption_panel) {
            let w = self.m.adaption;
            let rect = self.node_rect(self.m.adaption_itemframe);
            out.adaption = Some(adaption::frame(game, &self.adaption, self.local_cursor(w), self.widget_size(w), rect));
        }
        if self.visible(self.m.voxel_panel) {
            let w = self.m.voxel;
            let (model, model_index, ray) = match &inp.voxel {
                Some(v) => (v.model, v.model_index, v.ray),
                None => (None, None, None),
            };
            let vi = voxel::VoxelInputs {
                origin: self.widget_origin(w),
                size: self.widget_size(w),
                cursor,
                time_ms: inp.time_ms,
                model,
                model_index,
                ray,
            };
            out.voxel = Some(voxel::frame(&mut self.voxel, game, &vi));
        }
        // 0x0048bd8c..0x0048c100: the quick bar's icons and cooldowns.
        skill_panel::skill_bar_frame(self, &game.skill_bar);
        // 0x0048d771..0x0048f17e: the preview's item, the skills panel, the placement.
        let (source, item, mut hovered) = item_preview::item_source(self, game, inp.ground_item);
        let has_item = item.is_some();
        self.set_visible(self.m.preview, has_item);
        self.preview.skill = -1;
        self.preview.spec = -1;
        let hover = skill_panel::panel_frame(self, game, &mut out.node_colors);
        skill_panel::menu_bar_frame(self);
        if hover.hovered {
            // 0x0048e2b3 / 0x0048e56a: the preview shows the hovered button.
            self.preview.skill = hover.skill;
            self.preview.level = hover.level;
            self.preview.spec = hover.spec;
            self.set_visible(self.m.preview, true);
            hovered = true;
        }
        item_preview::place_previews(self, hovered);
        self.preview.equipped = item.as_deref().map_or(Vec::new(), |it| item_preview::equipped_list(game, source, it));
        self.preview.item = item;
        self.preview.source = source;
        let eq = !self.preview.equipped.is_empty() && self.visible(self.m.preview);
        self.set_visible(self.m.equipped_preview, eq);
        out.preview = item_preview::widget_frame(self);
        if self.visible(self.m.skills_panel) {
            let w = self.m.skills;
            out.skill_texts = skill_panel::widget_texts(self, game, self.local_cursor(w), self.widget_size(w));
        }
        // `SpeechWidget::update` 0x004e5f90 of the dialog (+0x800938): the typing sound's
        // `rand()` is drawn in the engine's widget pass, which `render` runs after `update`,
        // so after every `rand()` of the update half above (the craft bonus draws of
        // `crafting::update_timer`).
        if self.visible(self.m.speech_node) {
            let revealed = self.speech.revealed();
            let (sound, show) = self.speech.update(revealed, inp.rng);
            out.actions.extend(sound);
            out.speech_options = show;
        }
        // The same update of each shown speech bubble (`GC+0x80092c[i]`, [`bubbles`]): the
        // typing sound every third revealed character.
        let bubble_nodes: Vec<cw_ui::widget::NodeId> = self.hud_state.nodes.as_ref().map(|h| h.bubbles.iter().map(|b| b.0).collect()).unwrap_or_default();
        for (i, n) in bubble_nodes.into_iter().enumerate() {
            if !gui_models::shown(&self.gui, n) {
                continue;
            }
            if let Some(b) = self.bubbles.get_mut(i) {
                let revealed = b.revealed();
                let (sound, _) = b.update(revealed, inp.rng);
                out.actions.extend(sound);
            }
        }
        if game.map_open {
            if let Some(mi) = &inp.map {
                let names = map_overlay::NameSource { text: text.as_deref(), seeds: world.map(|w| &w.seeds) };
                out.map_overlay = Some(map_overlay::map_overlay(mi, &names));
            }
        }
        out
    }

    /// The left-button panel order of `onMouseDown` 0x0047b600 after the start menu: the
    /// crafting selection (0x0047bb65) and recipe click, the customization click, the
    /// inventory (equipment then bag), adaption, identification. `over_widget` = the engine
    /// hit a widget (0x00650ae0); `hovered_equipment` the equipment slot under the cursor
    /// ([`inventory::slot::HOVER_ORDER`]); `shift` = `GC+0x29`.
    fn left_panels(
        &mut self,
        game: &mut GameView,
        rng: &mut MsvcRand,
        over_widget: bool,
        hovered_equipment: Option<usize>,
        shift: bool,
        out: &mut Vec<UiAction>,
    ) {
        if self.visible(self.m.crafting_panel) {
            let size = self.widget_size(self.m.crafting);
            let (a, _) = self.inv.crafting.select(true, size);
            out.extend(a);
            let sel = (self.inv.crafting.selected_tab, self.inv.crafting.selected_index);
            crafting::on_recipe_click(&self.gui, &self.m, &mut self.crafting, sel);
        }
        voxel::left_click(&self.gui, &self.m, &mut self.voxel, game, over_widget);
        let ctx = inventory::ClickContext {
            panels: self.panels(),
            bag: &self.inv.bag,
            shop_widget: &self.inv.shop,
            hovered_equipment,
            shift,
        };
        out.extend(inventory::left_click(game, &ctx));
        let (c, size) = (self.local_cursor(self.m.adaption), self.widget_size(self.m.adaption));
        out.extend(adaption::adapt_click(&self.gui, &self.m, &self.adaption, game, c, size));
        // 0x0047c806..0x0047c8ea: Learn / Cancel.
        let (c, size) = (self.local_cursor(self.m.skills), self.widget_size(self.m.skills));
        skill_panel::learn_or_cancel(self, game, c, size);
        let (c, size) = (self.local_cursor(self.m.enchant), self.widget_size(self.m.enchant));
        out.extend(enchant::identify_click(&self.gui, &self.m, &mut self.enchant, game, rng, c, size));
    }

    fn panels(&self) -> inventory::Panels {
        inventory::Panels {
            inventory: self.visible(self.m.inventory_panel),
            shop: self.visible(self.m.shop_panel),
            customization: self.visible(self.m.voxel_panel),
            identification: self.visible(self.m.enchant_panel),
            adaption: self.visible(self.m.adaption_panel),
            equipment: self.flag_8008f0,
        }
    }

    /// `onMouseDown` 0x0047b600, UI part: the start menu, the right click on nothing
    /// (`over_widget` = the engine hit a widget, 0x00650ae0), the menu bar
    /// (`hovered_menu_button` = index into `menu_buttons`), the select-screen drag reset,
    /// the panels' left clicks ([`GameUi::left_panels`] order) and the right/middle clicks
    /// (the customization prelude 0x0047bab1, then [`inventory::right_click`], then the
    /// recipe rebuild 0x0047dd88).
    #[allow(clippy::too_many_arguments)]
    pub fn on_mouse_down(
        &mut self,
        game: &mut GameView,
        button: i32,
        over_widget: bool,
        hovered_menu_button: Option<usize>,
    ) -> Vec<UiAction> {
        let mut rng = MsvcRand::new(1);
        self.on_mouse_down_with(game, &mut rng, button, over_widget, hovered_menu_button, None, false, false)
    }

    /// [`GameUi::on_mouse_down`] with the CRT `rand()` stream, the hovered equipment slot,
    /// shift (`GC+0x29`) and `GC+0x800704` (build mode: only the ctor writes it, 0, so callers pass
    /// `false`; see `crate::interact::build_r`).
    #[allow(clippy::too_many_arguments)]
    pub fn on_mouse_down_with(
        &mut self,
        game: &mut GameView,
        rng: &mut MsvcRand,
        button: i32,
        over_widget: bool,
        hovered_menu_button: Option<usize>,
        hovered_equipment: Option<usize>,
        shift: bool,
        flag_800704: bool,
    ) -> Vec<UiAction> {
        let mut out = Vec::new();
        previews::on_mouse_down(&self.gui, &mut self.previews);
        flow::start_menu_click(self, game, button, &mut out);
        if button == 1 && !over_widget {
            flow::right_click_nothing(self);
        }
        flow::menu_bar_click(self, game, hovered_menu_button, button);
        // 0x0047b849..0x0047baab: the skills panel's buttons.
        out.extend(skill_panel::on_mouse_down(self, game, button));
        if button == 0 {
            self.left_panels(game, rng, over_widget, hovered_equipment, shift, &mut out);
        } else if button == 1 || button == 2 {
            voxel::right_click_prelude(&self.gui, &self.m, &mut self.voxel, flag_800704);
            let ctx = inventory::ClickContext {
                panels: self.panels(),
                bag: &self.inv.bag,
                shop_widget: &self.inv.shop,
                hovered_equipment,
                shift,
            };
            out.extend(inventory::right_click(game, &mut self.inv.shop_data, &ctx, button));
        }
        out
    }

    /// `onMouseUp` 0x0047ddd0, UI part, in its order: the identification (0x0047de35) and
    /// adaption (0x0047de8f) panels' "Goodbye!", then the system menu (the reader of
    /// `SystemWidget+0x160`, 0x0047e00e). `map_release` is the map screen's release between
    /// them (0x0047df01..0x0047e008, `crate::map_screen::on_release`), run by the controller.
    pub fn on_mouse_up(&mut self, game: &mut GameView, button: i32, map_release: &mut dyn FnMut(&mut GameView)) -> Vec<UiAction> {
        let mut out = Vec::new();
        let (c, size) = (self.local_cursor(self.m.enchant), self.widget_size(self.m.enchant));
        enchant::on_mouse_up(&mut self.gui, &self.m, &mut self.enchant, c, size);
        let (c, size) = (self.local_cursor(self.m.adaption), self.widget_size(self.m.adaption));
        adaption::on_mouse_up(&mut self.gui, &self.m, &mut self.adaption, c, size);
        map_release(game);
        flow::system_menu_click(self, game, button, &mut out);
        out
    }

    /// The controller's follow-up of the actions the UI itself owns: [`UiAction::RefreshPreviews`]
    /// (rebuild both select screens), [`UiAction::RebuildRecipes`] (0x004a14c0) and
    /// [`UiAction::SetItemTarget`] (the target of the adaption, identification or
    /// customization widget). Other actions are the controller's.
    pub fn handle_own_action(&mut self, game: &GameView, a: &UiAction) {
        match a {
            UiAction::RefreshPreviews => {
                previews::rebuild_character_previews(&mut self.gui, &mut self.m, &mut self.previews, game);
                previews::rebuild_world_previews(&mut self.gui, &self.m, &mut self.previews, game);
            }
            UiAction::RebuildRecipes => {
                let sel = (self.inv.crafting.selected_tab, self.inv.crafting.selected_index);
                crafting::refresh(&mut self.crafting, game, sel);
            }
            UiAction::SetItemTarget { panel, item } => match panel {
                inventory::TargetPanel::Adaption => adaption::set_target(&mut self.adaption, *item),
                inventory::TargetPanel::Enchant => enchant::set_target(&mut self.enchant, *item),
                inventory::TargetPanel::Voxel => voxel::set_target(&mut self.voxel, *item),
            },
            _ => {}
        }
    }
}

/// What [`GameUi::frame`] produced for the renderer and the controller: what is visible and
/// with what content (node visibility itself is written into the [`Gui`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameOutput {
    /// Actions of the frame (disconnect, camera orbit, craft results).
    pub actions: Vec<UiAction>,
    /// The select screens.
    pub previews: previews::PreviewsFrame,
    /// The start menu items (when its node is visible).
    pub start_menu: Vec<start_menu::MenuItem>,
    /// The system menu items (when its panel is visible).
    pub system_menu: Vec<start_menu::MenuItem>,
    /// The character style rows (in the creation screen).
    pub style_rows: Vec<character_style::StyleRow>,
    /// The options values (when the panel is visible).
    pub option_values: Option<[String; 11]>,
    /// `smallquesttag` / `questbar` (with quest inputs).
    pub quest_tag: Option<target::QuestTagFrame>,
    /// The craft bar's `bar` x scale.
    pub craft_bar_fill: Option<f32>,
    /// The HUD bars (with HUD inputs).
    pub hud_bars: Option<hud::HudBars>,
    /// The crosshairs and their text (with crosshair inputs).
    pub crosshair: Option<target::CrosshairFrame>,
    /// The nameplates (clones of the `:small` target bars into `GC+0x8007b0`).
    pub nameplates: Vec<target::PlateFrame>,
    /// The objective widget.
    pub objective: objective::ObjectiveFrame,
    /// The statistics widget (empty in this build).
    pub statistics: statistics::StatisticsFrame,
    /// The cursor node's caption (`GC+0x8008a8`): the held count or the customization pick.
    pub cursor_caption: String,
    /// The bag `InventoryWidget` (inventory panel visible).
    pub bag: Option<inventory_widget::InventoryFrame>,
    /// The crafting `InventoryWidget` (crafting panel visible).
    pub crafting: Option<inventory_widget::InventoryFrame>,
    /// The shop `InventoryWidget` (shop panel visible).
    pub shop: Option<inventory_widget::InventoryFrame>,
    /// The `BlueprintPreviewWidget` (craft preview panel visible).
    pub craft_preview: Option<crafting::BlueprintFrame>,
    /// The `EnchantWidget` (identification panel visible).
    pub enchant: Option<enchant::EnchantFrame>,
    /// The `AdaptionWidget` (adaption panel visible).
    pub adaption: Option<adaption::AdaptionFrame>,
    /// The `VoxelWidget` (customization panel visible).
    pub voxel: Option<voxel::VoxelFrame>,
    /// The map overlay labels (map open, with map inputs).
    pub map_overlay: Option<map_overlay::MapOverlayFrame>,
    /// The dialog `SpeechWidget` shows its answer options (the page has had its time).
    pub speech_options: bool,
    /// Display fill colours written this frame (the skill buttons' `frame` children).
    pub node_colors: Vec<(cw_ui::widget::NodeId, [f32; 4])>,
    /// `PreviewWidget::update` 0x004d50a0's non-text output.
    pub preview: item_preview::PreviewFrame,
    /// `SkillWidget::update` 0x004dd810's texts (skills panel visible).
    pub skill_texts: Vec<crafting::PanelText>,
}
