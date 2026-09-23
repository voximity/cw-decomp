//! `cube::GameController` (`Cube.exe`, vtable 0x006ffdc8, 0x1001018 bytes): the constructor
//! 0x00459c40, `update(dt)` 0x00488ee0 and `render()` 0x004ac260, as [`Controller`].
//!
//! Tier B for the flow and the order of operations; the pieces the other modules port are
//! called here in the original's order. The controller owns the state the original keeps in
//! its 16 MB object; each field names its offset.
//!
//! # Map of `update` 0x00488ee0 (77 KB)
//!
//! | Range | Piece | Here |
//! |---|---|---|
//! | 0x00488ee0..0x00489020 | frame time, `+0x8006e8` | [`Controller::update`] head |
//! | 0x00489020..0x00489372 | the distance to the nearest unmeshed chunk, eased into `+0x1d4` | [`Controller::fog_step`] |
//! | 0x00489372..0x00489ca0 | the screen rules | `ui::GameUi::frame` (`ui::flow::frame_rules`) |
//! | 0x0048b8ab..0x0048ce2e | skill bar, dead latch, pickups | `player::LocalPlayer::before_tick` |
//! | 0x0048ce2e..0x0048cea3 | `+0x8006e8 = dt`; the in-process server's `takeHits`/`takePassives`/`takeShoots` (never present in this build, see `singleplayer.rs`) | – |
//! | 0x0048cea3..0x0048cf4e | world match test; `World::tick 0x0060c510` in 20 ms slices with a NULL received list (the client's world is a server world in singleplayer, `singleplayer.rs`) | [`Controller::world_tick`] |
//! | 0x0048d5f0..0x0049081b | the received update lists (`+0x800630`) applied and spliced into the tick's output | `received::apply` in [`Controller::simulate`] |
//! | 0x00491f16..0x0049ced6 | the gameplay ranges (quick items, XP display, camera ease, R/E/T, movement, aim, attacks, camera) | `player::LocalPlayer::after_tick` |
//! | 0x004952a5..0x004955b1 | server particles | `particles::ParticleSystem::spawn_server` in [`Controller::apply_received`] |
//! | 0x00495630.. | server sounds (camera shake on 0x51/0x52) | [`Controller::apply_received`] |
//! | 0x00495f25..0x00496329 | particle update | `particles::ParticleSystem::update` |
//! | 0x004963cb..0x0049645b | flash lights aged | `scene::FlashLight::advance` |
//! | 0x0049c02f..0x0049c254 | the tick's own hits/passives/shoots to the send queues | `net::NetWorld::queue_tick_output` |
//! | 0x0049cee0..0x0049d110 | under `+0x8005d0`: chunk window origin, player block, zone centres | [`Controller::publish_thread_inputs`] |
//!
//! Audio housekeeping (`reapFinishedVoices`, `streamMusic`, the music volume fade) has no
//! counterpart: kira frees voices, there is no music (`audio.rs`).
//!
//! # Map of `render` 0x004ac260 (59 KB)
//!
//! [`Controller::render`]: `scene::gather_scene` (every `rand()` side effect of the original's
//! render, in order, before the frame), the creature bodies (`cw_render::pose`, the pose
//! object `creature+0x1498` per creature), `cw_render::passes::build_frame`, then the device
//! work through a [`FrameSink`] (the wgpu executor).

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use cw_math::rand::MsvcRand;
use cw_net::EntityData;
use cw_net::packet::ServerUpdate;
use cw_render::frame::{D3dMatrix, FrameCommands, ModelRef};
use cw_render::mesh::{ChunkBuild, VoxelGrid};
use cw_render::passes::{self, AfterimageFields, ChunkBufferInput, ChunkInput, CreatureInput, FrameMode, ModelDraw, RenderInputs, WorldSampler};
use cw_render::pose::{self, PoseInputs, PoseState, WalkCycle};
use cw_sim::combat::CreatureState;
use cw_sim::projectile::Projectile;
use cw_ui::widget::Gui;
use cw_world::World;
use cw_world::inventory::Item;

use crate::audio::{AudioEngine, Listener};
use crate::input::{self, ControllerBytes, InputState, KeyDown, KeyGates, MouseButton, VkState};
use crate::interact::{self, ItemModels};
use crate::landscape::{LandscapeWorker, landscape_centre};
use crate::net::{self, NetShared};
use crate::options::{OPTIONS_FILE, Options};
use crate::particles::ParticleSystem;
use crate::player::{self, CameraOptions, Event, FrameInput, LocalPlayer, UiState};
use crate::profile::{self, Phase};
use crate::scene::{self, ModelInfo, SceneChunk, SceneModels, SceneState};
use crate::persist::{self, CharacterRecord, RecordDb, WorldRecord};
use crate::received;
use crate::singleplayer::{self, MapBlobs};
use crate::threads::{ClientShared, LOCAL_PLAYER_ID, Workers};
use crate::ui::{self, GameUi, GameView, UiAction, plx_files::GamePlxLoader};

/// The world tick's step (`update` splits `dt` into 20 ms slices, then the remainder).
pub const TICK_MS: i32 = 20;

/// Model handle of the particle cube `GC+0x800730` (a 1x1x1 voxel model the constructor
/// builds; not in the model table).
pub const PARTICLE_CUBE: ModelRef = 0x3fff_fff0;

// ---------------------------------------------------------------------------------------------
// The model cache (`GC+0x300`).

/// The controller's model vector (`GC+0x300`, loaded by the constructor from `data1.db`
/// through `Model::load`), shared with the world (`world+0x20`). A model's handle is its
/// index.
pub struct ModelCache {
    pub models: Arc<Vec<cw_world::model::Model>>,
}

impl ModelCache {
    fn info(&self, index: i32) -> Option<ModelInfo> {
        let m = self.models.get(usize::try_from(index).ok()?)?;
        if m.voxels.is_empty() {
            return None;
        }
        Some(ModelInfo { handle: index as ModelRef, size: m.size })
    }
}

impl SceneModels for ModelCache {
    fn model(&self, index: i32) -> Option<ModelInfo> {
        self.info(index)
    }
    /// `GC+0x800718`: the prop model table (`crate::assets::PROP_MODELS`, filled by the ctor
    /// at 0x00462563..0x00462bf9 from the model vector); the null slots are `None`.
    fn prop_model(&self, kind: u32) -> Option<ModelInfo> {
        let idx = *crate::assets::PROP_MODELS.get(kind as usize)?;
        if idx < 0 {
            return None;
        }
        self.info(idx)
    }
    fn prop_model_count(&self) -> usize {
        crate::assets::PROP_MODELS.len()
    }
    fn item_model(&self, item: &Item) -> Option<ModelInfo> {
        let idx = interact::item_model_index(&item.to_bytes())?;
        self.info(idx as i32)
    }
    fn voxels(&self, model: ModelRef) -> Option<VoxelGrid<'_>> {
        let m = self.models.get(model as usize)?;
        Some(VoxelGrid { size: m.size, voxels: &m.voxels })
    }
    fn particle_cube(&self) -> ModelRef {
        PARTICLE_CUBE
    }
}

impl pose::ModelSource for ModelCache {
    fn item_model(&self, item: &[u8]) -> Option<u32> {
        interact::item_model_index(item)
    }
    fn model_size(&self, model: u32) -> [i32; 3] {
        self.models.get(model as usize).map_or([0; 3], |m| m.size)
    }
    fn model_count(&self) -> usize {
        self.models.len()
    }
}

impl ui::gui_models::GuiModelSource for ModelCache {
    fn size(&self, index: u32) -> Option<[i32; 3]> {
        self.info(index as i32).map(|m| m.size)
    }
    fn spirit_cube(&self) -> ModelRef {
        PARTICLE_CUBE
    }
}

impl ItemModels for ModelCache {
    fn size(&self, model_index: u32) -> Option<[i32; 3]> {
        self.models.get(model_index as usize).map(|m| m.size)
    }
}

/// `cw_render::passes::WorldSampler` over the client's world (the orphan rule needs the
/// wrapper).
pub struct WorldView<'a>(pub &'a World);

impl WorldSampler for WorldView<'_> {
    fn climate_a(&self, x: i32, y: i32) -> f32 {
        self.0.climate_a(x, y)
    }
    fn climate_b(&self, x: i32, y: i32) -> f32 {
        self.0.climate_b(x, y)
    }
    fn climate_c(&self, x: i32, y: i32) -> f32 {
        self.0.climate_c(x, y)
    }
    /// `0x005f0720`: the water fraction of the climate blend.
    fn climate_d(&self, x: i32, y: i32) -> f32 {
        self.0.climate_water(x, y)
    }
    fn mountain_factor(&self, x: i32, y: i32) -> f32 {
        self.0.mountain_factor(x, y)
    }
    fn base_height(&self, x: i32, y: i32) -> f32 {
        self.0.base_height(x, y)
    }
    fn value_noise_2d(&self, x: f64, y: f64) -> f32 {
        cw_math::noise::value_noise_2d(x, y)
    }
}

// ---------------------------------------------------------------------------------------------
// The device seam.

/// What the executor gets besides the frame: the meshes behind the handles the frame names.
pub struct FrameResources<'a> {
    /// Per ring record (the index of `RenderInputs::chunks`): the chunk's slab meshes, with the
    /// record's version for re-uploads.
    pub chunks: &'a [(u64, Option<Arc<ChunkBuild>>)],
    /// The model table (`ModelRef` = index; [`PARTICLE_CUBE`] is the unit cube).
    pub models: &'a ModelCache,
    /// Map tile meshes added (`Some`) or released (`None`) since the last frame
    /// (`cw_render::map::MapTiles::take_mesh_events`).
    pub map_meshes: &'a [(ModelRef, Option<cw_render::mesh::ModelMesh>)],
    /// The GUI stream, render surfaces, glyph atlas and pending assets of this frame.
    pub upload: crate::assets::FrameUpload,
}

/// The device side of `render` and `frame` 0x004c85f0 (Present), implemented by the wgpu
/// executor (`app::ExecSink`; `app::NullSink` without a GPU).
pub trait FrameSink {
    /// Draws and presents one frame.
    fn render(&mut self, frame: &FrameCommands, resources: FrameResources<'_>);
    /// `resetDevice 0x004c8940`: the back buffer's new size.
    fn resize(&mut self, width: u32, height: u32);
    /// `resetDevice 0x004c8940`: the anti-aliasing option (`0x0076b1e4`, `level * 2` samples,
    /// 0 off).
    fn set_anti_aliasing(&mut self, _level: i32) {}

    /// Read the next presented frame back (the port's `--shot-at`; nothing by default).
    fn request_capture(&mut self) {}

    /// The frame read back since [`FrameSink::request_capture`]: `(width, height, RGBA8)`.
    fn take_capture(&mut self) -> Option<(u32, u32, Vec<u8>)> {
        None
    }

    /// Port-only debug overlay (`debug_overlay.rs`): its renderer, drawn after the frame.
    #[cfg(feature = "debug-overlay")]
    fn debug_overlay(&mut self) -> Option<&mut crate::debug_overlay::OverlayRenderer> {
        None
    }
}

// ---------------------------------------------------------------------------------------------
// The connection.

/// `GC+0x8006cc` (the socket) and the network threads of a connection to a server. There is
/// no singleplayer connection: singleplayer is the client's own world (`singleplayer.rs`).
type Session = net::Connection;

/// `0x0076b040`: click-to-walk. Read at 0x00498078 and 0x0049b4c6; nothing in Cube.exe writes
/// it (an absolute-address scan of `.text`), so it stays 0 and [`Controller::click_to_walk`]
/// never runs.
pub const CLICK_TO_WALK: bool = false;

/// The period of the mesher's character save (0x004690a0: every 60 s, 0x00487520).
pub const CHARACTER_SAVE_MS: i32 = 60_000;

// ---------------------------------------------------------------------------------------------
// The controller.

/// `cube::GameController`.
pub struct Controller {
    // --- cube::Controller base (0x0043b4c0) ---
    /// `+0x4..+0x17`, `+0x124..+0x130`: what the device layer reported this frame (WinMain
    /// copies it into the controller bytes before `update`).
    pub input: InputState,
    /// The image of `+0x4..+0x18` and `+0x124..+0x130` `update` reads.
    pub bytes: ControllerBytes,
    /// `+0x19[256]`, `+0x119[3]`: VK key and button state.
    pub vk: VkState,
    /// `+0x11c`, `+0x120`: client width and height.
    pub screen: [i32; 2],
    /// Port-only: the back buffer the GUI is drawn into, in device pixels, and the device
    /// pixels per [`Self::screen`] unit (the window's DPI scale when windowed, see `app.rs`),
    /// set by [`Self::set_back_buffer`]. `[0, 0]` is the screen size at scale 1.
    pub back_buffer: [u32; 2],
    pub pixel_scale: f32,

    // --- options and flags ---
    /// `+0x170..+0x19c`: the options block (mirror of 0x0076b1d8).
    pub options: Options,
    /// `+0x1a0`: quit flag (WinMain posts WM_QUIT).
    pub quit: bool,
    /// `+0x1d0`: the prop view distance, `window * 0.5 * 32` (0x0045ab13).
    pub view_distance: f32,
    /// `+0x1d4`: the eased distance to the nearest unmeshed chunk, the fog distance.
    pub fog_distance: f32,
    /// `+0x800598`: the server address (`/connect`; argv[1] of WinMain presets it).
    pub server_address: String,
    /// WinMain's `server` flag (argv[1] == "server"): passed to the constructor, which never
    /// reads it (its argument slot `[ebp+0x20]` has no reader in 0x00459c40).
    pub server_flag: bool,

    // --- the world and its threads ---
    /// Locks A, B, C and the world (`+0x2e4`), ring (`+0x2e0`), map tiles (`+0x800d44`).
    pub shared: Arc<ClientShared>,
    /// Lock A's creature map and the network lists.
    pub net: Arc<NetShared>,
    workers: Workers,
    landscape: Option<LandscapeWorker>,
    /// `+0x8006cc` and the send/receive thread handles `+0x800590`/`+0x800594`.
    session: Option<Session>,
    /// The world whose map cache is loaded (`Save/map_<name>.db`).
    map_world: Option<String>,
    /// The map record parts `MapTiles` does not model (`singleplayer::MapBlobs`).
    map_blobs: MapBlobs,
    /// `world+0x8000bc` and the rest of the tick's clock (`cw_sim::day`).
    clock: cw_sim::day::Clock,
    /// `creature+0x130c` of the local player: the interactions the next tick dispatches.
    pending_interacts: Vec<cw_net::packet::Interact>,
    /// `zone+0x3c` per zone (the received `items_8` lists).
    items8: BTreeMap<(i32, i32), Vec<cw_net::packet::Item8>>,
    /// The last day and time of packet 5 applied to the world.
    applied_time: Option<(u32, u32)>,
    /// `GC+0x1001008`: `Save/characters.db`.
    char_db: Option<RecordDb>,
    /// `GC+0x1001010`: `Save/worlds.db`.
    world_db: Option<RecordDb>,
    /// `GC+0x800984`: the saved characters' creatures, as records.
    characters: Vec<CharacterRecord>,
    /// `GC+0x8009dc`: the saved worlds' preview models (`WorldInfo+4`), by list index.
    world_previews: Vec<Option<([i32; 3], Vec<u8>)>>,
    /// The saved characters' creatures the select screens pose (`GC+0x800984`,
    /// `ui::previews::PreviewModels`).
    preview_models: ui::previews::PreviewModels,
    /// `creature+0x119c` of the local player (kept for the character record).
    player_f119c: u32,
    /// `0x0076b078`: `timeGetTime()` of the mesher's last character save.
    last_character_save: u32,
    /// Chat lines and sounds the received records asked for (`received::Effect`), handled
    /// after the world lock.
    effects: Vec<received::Effect>,

    // --- the local player, camera, simulation ---
    /// `+0x8006d0` and the camera fields `+0x140..+0x26c`.
    pub player: LocalPlayer,
    /// `creature+0x1188`/`+0x118c` per creature.
    pub walk: BTreeMap<i64, WalkCycle>,
    /// `creature+0x1498` per creature: the pose object.
    pub poses: BTreeMap<i64, PoseState>,
    /// `world+0x14` (`GC+0x2f8`): the projectiles of the client's tick.
    pub projectiles: Vec<Projectile>,
    /// `+0x800740`: particles.
    pub particles: ParticleSystem,
    /// The fields `render` reads (`+0x140`, `+0x1d8`, `+0x2e0`, `+0x800748`, ...).
    pub scene: SceneState,
    /// `+0x8003a0` (`World+0x8000bc`): the millisecond clock.
    pub world_ms: i32,
    /// `+0x8006e8`: the last frame's duration.
    pub frame_ms: i32,
    /// Per ring record: the distance key `+0x6c` render keeps between frames.
    chunk_keys: Vec<f32>,
    /// Milliseconds counted for `CW_CLIENT_STATS`.
    stats_ms: i32,
    /// Zones the tick changed, to mark for remeshing after the world lock is released.
    dirty_zones: std::collections::BTreeSet<(i32, i32)>,
    /// The join asked for every chunk to be remeshed.
    remesh_all: bool,
    /// `cube::WorldMap` (`GC+0x800d44`): the fields that carry over between draws.
    pub map_state: cw_render::map::MapState,
    /// The models the map draws (the world model table by slot).
    map_models: cw_render::map::MapModels,

    // --- audio and UI ---
    /// `+0x800714`: the audio engine.
    pub audio: AudioEngine,
    /// The widget tree and per-widget state (`+0x800874..+0x800ad4`).
    pub ui: GameUi,
    /// `GC+0x1000e4c..+0x1000e54`: the map pan in blocks: zero in the ctor (0x0045a5b7), the
    /// middle-button drag while the map is open and zero while it is closed (`update`
    /// 0x0048b627..0x0048b802, [`crate::map_screen::update_pan`]).
    pub map_pan: [f32; 3],
    /// `GC+0x800de4`: the map was opened from a bed or teleporter (targets are pickable);
    /// cleared while the map is closed (0x00497347).
    pub teleport_map: bool,
    /// `GC+0x800dd4` (hovered) and `GC+0x800ddc` (picked): the teleporter zones of the map
    /// screen ([`crate::map_screen`]).
    pub teleport_pick: crate::map_screen::TeleportPick,
    /// `GC+0x800a48` / `+0x800a4c`: the Tab quick-item wheel's radius and angle (`render`
    /// 0x004bac91..0x004bb039).
    pub quick_wheel: ui::quick_item::QuickWheel,
    /// `GC+0x80075c`: the stars (0x004636a8..0x0046385f, `crate::assets::stars`).
    pub stars: Vec<[f32; 4]>,
    /// The GUI's font engine and glyph atlas, the ctor's device assets waiting for the sink.
    pub assets: crate::assets::AssetState,
    /// The style the local player's appearance was last built from.
    applied_style: ui::Style,
    /// The last `GameUi::frame` output (what the widgets show; `ui::present`).
    pub ui_out: ui::FrameOutput,
    /// The client state the UI reads.
    pub game: GameView,
    /// The loaded `.plx` scenes.
    pub plx: GamePlxLoader,
    /// `+0x300`: the model vector.
    pub models: ModelCache,
    /// The game folder (the original's working directory).
    pub game_dir: PathBuf,
}

/// The game folder: `CW_GAME_DIR`, else `game` under the working directory, else the working
/// directory (the original runs from its own folder).
pub fn default_game_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("CW_GAME_DIR") {
        return PathBuf::from(d);
    }
    let g = PathBuf::from("game");
    if g.join("start.plx").exists() {
        return g;
    }
    // The workspace layout: crates/cw-client → ../../game.
    let w = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../game");
    if w.join("start.plx").exists() {
        return w;
    }
    PathBuf::from(".")
}

impl Controller {
    /// `GameController::GameController` 0x00459c40, with WinMain's steps that feed it: the
    /// options (`Options::load` 0x004ce6e0 over the screen size), the audio engine
    /// (`XAudio2Engine` 0x00622da0 opening `data2.db`), the model vector from `data1.db`, the
    /// widget tree (`ui::GameUi::new` with the five `.plx` files), the local creature, the
    /// client world, the three world threads, and the start screen.
    pub fn new(game_dir: PathBuf, screen: [i32; 2], display_modes: Vec<(i32, i32)>, server_flag: bool, server_address: String) -> Controller {
        // WinMain 0x004c8ae0 step 7: defaults, the screen size, options.cfg.
        let mut options = Options { resolution_x: screen[0], resolution_y: screen[1], ..Options::default() };
        options.load(game_dir.join(OPTIONS_FILE));
        // Step 8: the audio engine over data2.db.
        let sounds = cw_formats::AssetDb::open(game_dir.join("data2.db")).ok();
        let audio = AudioEngine::new(sounds);
        // The model vector (`GC+0x300`, data1.db).
        let mut models = Vec::new();
        if let Err(e) = cw_world::model::load_models_from_game_dir(&game_dir, &mut models) {
            eprintln!("cw-client: model table: {e}");
        }
        let models = Arc::new(models);
        // The GUI (0x0045a746..0x004655..).
        let mut game = GameView { options, display_modes: display_modes.clone(), ..GameView::default() };
        let mut gui = Gui::new();
        gui.viewport = glam::IVec2::new(screen[0], screen[1]);
        let mut plx = GamePlxLoader::new(&game_dir);
        let ui = GameUi::new(gui, &mut plx, &game);
        for (f, e) in &plx.errors {
            eprintln!("cw-client: {f}: {e}");
        }
        eprintln!(
            "cw-client: game folder {}, {} models, loaded {:?}",
            game_dir.display(),
            models.len(),
            plx.loaded.iter().map(|l| l.file.as_str()).collect::<Vec<_>>()
        );
        // The local creature (+0x8006d0, 0x004620..0x0046222c): the player entity, in the
        // creature map under id 1 and set as the world's local player (`world+0xb8`).
        let net = NetShared::new();
        {
            let mut nw = net.world.lock().unwrap_or_else(|e| e.into_inner());
            nw.local_player = LOCAL_PLAYER_ID;
            nw.entities.insert(LOCAL_PLAYER_ID, fresh_player_entity());
            nw.states.insert(LOCAL_PLAYER_ID, fresh_player_state());
        }
        game.player = net.world.lock().unwrap_or_else(|e| e.into_inner()).entities[&LOCAL_PLAYER_ID].clone();
        let shared = Arc::new(ClientShared::new(Arc::clone(&net), Arc::clone(&models), game_dir.clone(), options.render_distance));
        // 0x0045a72d..0x0045a741: the start screen's world. `GC+0x800a50 = timeGetTime()`, then
        // `World::load(GC+0x2e4, that, GC+0x800a54 = "")` 0x005a52e0 (whose `srand(seed)` at
        // 0x005a532a seeds the global `rand` stream). A different world every launch. The
        // original loads it on the constructor's thread; the port's zone thread does it on its
        // first pass (the request does not match the empty `loaded`).
        let start_seed = net::time_get_time() as i32;
        {
            let mut cs = shared.lock_world_cs();
            cs.requested_seed = start_seed;
            cs.requested_name.clear();
        }
        net.world.lock().unwrap_or_else(|e| e.into_inner()).world_seed = start_seed as u32;
        // The three world threads (mesh, landscape, zone), in the constructor's order.
        let workers = Workers::start(&shared);
        let landscape = Some(LandscapeWorker::spawn(Arc::clone(&shared.world), Arc::clone(&shared.gate), Arc::clone(&shared.tiles), [0, 0]));
        let mut c = Controller {
            input: InputState::default(),
            bytes: ControllerBytes::default(),
            vk: VkState::default(),
            screen,
            back_buffer: [0, 0],
            pixel_scale: 1.0,
            options,
            quit: false,
            view_distance: (crate::threads::CHUNK_WINDOW as f32) * 0.5 * 32.0,
            fog_distance: 0.0,
            server_address,
            server_flag,
            shared,
            net,
            workers,
            landscape,
            session: None,
            map_world: None,
            map_blobs: MapBlobs::default(),
            clock: cw_sim::day::Clock::default(),
            pending_interacts: Vec::new(),
            items8: BTreeMap::new(),
            applied_time: None,
            char_db: None,
            world_db: None,
            characters: Vec::new(),
            world_previews: Vec::new(),
            preview_models: ui::previews::PreviewModels::default(),
            player_f119c: 0,
            // 0 in the original against the system uptime (more than 60 s): the first check passes.
            last_character_save: net::time_get_time().wrapping_sub(CHARACTER_SAVE_MS as u32 + 1),
            effects: Vec::new(),
            player: LocalPlayer::new(LOCAL_PLAYER_ID),
            walk: BTreeMap::new(),
            poses: BTreeMap::new(),
            projectiles: Vec::new(),
            particles: ParticleSystem::new(),
            scene: SceneState::default(),
            world_ms: 0,
            frame_ms: 0,
            chunk_keys: Vec::new(),
            stats_ms: 0,
            dirty_zones: std::collections::BTreeSet::new(),
            remesh_all: false,
            map_state: cw_render::map::MapState::default(),
            map_models: cw_render::map::MapModels {
                world: models.iter().enumerate().map(|(i, m)| (!m.voxels.is_empty()).then_some(cw_render::map::MapModel { model: i as ModelRef, size: m.size })).collect(),
                border_post: Some(cw_render::map::MapModel { model: crate::assets::BORDER_POST_MODEL, size: cw_render::map::border_post_voxels().0 }),
            },
            audio,
            ui,
            ui_out: ui::FrameOutput::default(),
            applied_style: ui::Style::default(),
            map_pan: [0.0; 3],
            teleport_map: false,
            teleport_pick: crate::map_screen::TeleportPick::default(),
            quick_wheel: ui::quick_item::QuickWheel::default(),
            stars: crate::assets::stars_for_seed(net::time_get_time()),
            assets: crate::assets::AssetState::default(),
            game,
            plx,
            models: ModelCache { models },
            game_dir,
        };
        // 0x004621ea: the local creature's appearance from its fresh style record.
        c.rebuild_player_appearance(false);
        c.on_resize(screen[0], screen[1]);
        // 0x00464f0c..0x00465111: `Save/characters.db` and `Save/worlds.db`, every record
        // loaded (0x004806c0 into a new creature each, 0x004809a0 into a new `WorldInfo`).
        c.open_saves();
        // 0x0045aad3..0x0045ab0d: the constructor's camera: distance and wheel distance 0, the
        // eased angles (120, 0, 0) copied to the targets (no easing up from pitch 0). The
        // start screen's orbit (0x00489cd3) then takes it to distance 10, pitch 100.
        {
            let cam = &mut c.player.camera;
            cam.dist = 0.0;
            cam.dist_target = 0.0;
            cam.rot = [120.0, cam.rot[1], 0.0];
            cam.rot_target = cam.rot;
        }
        c.apply_action(UiAction::RefreshPreviews);
        c
    }

    /// The constructor's loads of the two save databases (0x00464f0c..0x00465111).
    fn open_saves(&mut self) {
        match RecordDb::open(&persist::characters_path(&self.game_dir)) {
            Ok(db) => {
                let n = db.count();
                self.characters = (0..n).map(|i| db.get(i).map(|b| CharacterRecord::from_blob(&b)).unwrap_or_default()).collect();
                self.char_db = Some(db);
            }
            Err(e) => eprintln!("cw-client: {e}"),
        }
        match RecordDb::open(&persist::worlds_path(&self.game_dir)) {
            Ok(db) => {
                let n = db.count();
                let recs: Vec<WorldRecord> = (0..n).map(|i| db.get(i).map(|b| WorldRecord::from_blob(&b)).unwrap_or_default()).collect();
                self.game.worlds = recs.iter().map(|r| ui::WorldEntry { name: String::from_utf8_lossy(&r.name).into_owned(), seed: r.seed, explored: r.explored }).collect();
                self.world_previews = recs.into_iter().map(|r| r.preview).collect();
                self.world_db = Some(db);
            }
            Err(e) => eprintln!("cw-client: {e}"),
        }
        self.refresh_character_entries();
    }

    /// The select screen's view of the saved characters (`GC+0x800984`).
    fn refresh_character_entries(&mut self) {
        self.game.characters = self
            .characters
            .iter()
            .map(|c| {
                let t = persist_entity_type(c);
                ui::CharacterEntry { name: String::from_utf8_lossy(&c.name).into_owned(), level: c.level, class: c.class, specialization: c.specialization, entity_type: t }
            })
            .collect();
        let entities = self.characters.iter().map(preview_entity).collect();
        self.preview_models.set_characters(entities);
    }

    // -----------------------------------------------------------------------------------------
    // Controller slots 1..9

    /// Slot 1 `isCursorFree` 0x0047f1d0.
    pub fn is_cursor_free(&self) -> bool {
        ui::flow::is_cursor_free(&self.ui, &self.game)
    }

    fn key_gates(&self) -> KeyGates {
        let m = &self.ui.m;
        KeyGates {
            blocking_panel_open: self.ui.visible(m.char_create_root)
                || self.ui.visible(m.world_create_root)
                || self.ui.visible(m.char_select_root)
                || self.ui.visible(m.server_root)
                || self.ui.visible(m.world_select_root),
            text_focus: self.ui.gui.has_text_focus().is_some(),
            chat_active: self.ui.chat.input_active,
        }
    }

    /// Slot 2 `onKeyDown` 0x0047e1b0 (after the plasma hook 0x00652730 forwarded the key to
    /// the focused widget).
    pub fn on_key_down(&mut self, vk: u8) {
        self.ui.gui.inject_key_down(u16::from(vk));
        let gates = self.key_gates();
        match input::on_key_down(&mut self.vk, gates, vk) {
            KeyDown::Ignored => {}
            KeyDown::Chat { submit } => {
                if submit {
                    let actions = self.ui.chat.enter(&self.game);
                    for a in actions {
                        self.apply_action(a);
                    }
                } else {
                    let shift = self.vk.keys[0x10];
                    self.ui.chat.on_key(u16::from(vk), shift);
                }
            }
            KeyDown::Key(action) => {
                use input::KeyAction;
                match action {
                    Some(KeyAction::ToggleLamp) => {
                        let id = self.player.id;
                        if let Some(e) = self.net.world.lock().unwrap_or_else(|e| e.into_inner()).entities.get_mut(&id) {
                            player::toggle_lamp(e);
                        }
                    }
                    Some(KeyAction::UseSpecialItem) => {
                        let id = self.player.id;
                        let mut nw = self.net.world.lock().unwrap_or_else(|e| e.into_inner());
                        let ground = nw.states.get(&id).map_or(0.0, |s| s.ground_z);
                        if let Some(e) = nw.entities.get_mut(&id) {
                            player::use_special_item(e, ground);
                        }
                    }
                    Some(_) => {
                        ui::flow::on_key_down(&mut self.ui, &mut self.game, vk);
                        // Tab flips `GC+0x800a40`, which the gameplay's quick menu reads too.
                        self.player.quick_open = self.ui.flag_800a40;
                    }
                    None => {}
                }
            }
        }
    }

    /// Slot 3 `onKeyUp` 0x0047e9d0.
    pub fn on_key_up(&mut self, vk: u8) {
        self.ui.gui.inject_key_up(u16::from(vk));
        let focus = self.ui.gui.has_text_focus().is_some();
        input::on_key_up(&mut self.vk, focus, vk);
    }

    /// Port-only: the back buffer size in device pixels and the device pixels per client
    /// unit, for the GUI stream (`assets::gui_frame`): the GUI is laid out in client units
    /// and drawn, text included, at the back buffer's resolution.
    pub fn set_back_buffer(&mut self, width: u32, height: u32, scale: f64) {
        self.back_buffer = [width, height];
        self.pixel_scale = if scale > 0.0 { scale as f32 } else { 1.0 };
    }

    /// Slot 4 `onResize` 0x00482a40 (and `resetDevice` 0x004c8940 storing `+0x11c/+0x120`).
    pub fn on_resize(&mut self, w: i32, h: i32) {
        if w <= 0 || h <= 0 {
            return;
        }
        self.screen = [w, h];
        self.scene.screen = [w, h];
        self.ui.gui.viewport = glam::IVec2::new(w, h);
        let gc = self.ui.m.on_resize_widgets(&self.ui.gui);
        cw_ui::widget::game_controller_on_resize(&mut self.ui.gui, &gc, w, h);
    }

    /// Slot 5 `onChar` 0x00482a00 (after the plasma hook 0x00652710).
    pub fn on_char(&mut self, c: u16) {
        self.ui.gui.inject_char(c);
        if self.ui.chat.input_active && c >= 0x20 {
            self.ui.chat.on_char(c);
        }
    }

    /// Slot 6 `onMouseMove` 0x0047ea00: with the cursor free the GUI gets the absolute
    /// position; otherwise the camera turns by the relative motion.
    pub fn on_mouse_move(&mut self, x: f32, y: f32) {
        // 0x0047ea2b..0x0047ecc3, before the cursor-free test: the smoothed cursor y, and over
        // a select screen with the engine's left button (`Engine+0xf4` bit 0) the move is
        // injected, the carousel drags and the slot returns (`ui::previews::on_mouse_move`).
        let client = glam::IVec2::new(self.screen[0], self.screen[1]);
        let ui = &mut self.ui;
        if ui::previews::on_mouse_move(&mut ui.gui, &ui.m, &mut ui.previews, net::time_get_time(), client, |g| g.inject_mouse_move(x, y, None)) {
            self.ui.on_mouse_moved();
            return;
        }
        if self.is_cursor_free() {
            // 0x0047ec94: `Engine::mouseMove` 0x00652c10 (the engine's previous cursor
            // `+0xdc/+0xe0` takes the current one), then with no node under the cursor
            // (0x00650ae0) and the left button byte `GC+4` down, the camera turns by the
            // cursor's motion (`Camera::on_mouse_drag`, 0x0047ecab..0x0047ed7b). This is the
            // drag that turns the character in the creation screen (and every other
            // cursor-free state).
            self.ui.gui.inject_mouse_move(x, y, None);
            // The scroll buttons' `MOUSE_MOVE` callback (`dragScroll` 0x004c5bb0) runs inside
            // the engine's inject in the original.
            self.ui.on_mouse_moved();
            if self.ui.gui.current_target().is_none() && self.bytes.left_mouse() {
                let d = self.ui.gui.cursor - self.ui.gui.prev_cursor;
                let opts = self.camera_options();
                self.player.camera.on_mouse_drag(&opts, d.x, d.y);
            }
        } else {
            let opts = self.camera_options();
            self.player.camera.on_mouse_move(&opts, x, y);
        }
    }

    /// Slot 7 `onMouseDown` 0x0047b600 (UI part; the gameplay clicks are `update`'s).
    pub fn on_mouse_down(&mut self, b: MouseButton) {
        self.vk.mouse_down(b);
        match b {
            MouseButton::Left => self.ui.gui.inject_left_down(None),
            MouseButton::Right => self.ui.gui.inject_right_down(),
            MouseButton::Middle => {}
        }
        let over = self.ui.gui.hovered.is_some();
        // 0x0047b7a5: `Button::isHovered` 0x006294c0 (the button's node or a child under the
        // cursor).
        let hovered_button = ui::skill_panel::hovered_menu_button(&self.ui);
        // 0x0047b1b0: the first hovered equipment box (`GC+0x800a28[i]`, slot `HOVER_ORDER[i]`),
        // while `GC+0x8008f0`.
        let hovered_equipment = if self.ui.flag_8008f0 {
            ui::inventory::slot::HOVER_ORDER.iter().enumerate().find(|&(i, _)| {
                self.ui.m.equipment_sprites.get(i).is_some_and(|&w| self.ui.is_hovered(w))
            }).map(|(_, &k)| k)
        } else {
            None
        };
        let shift = self.vk.keys[0x10];
        self.mirror_to_view();
        // The CRT `rand()` of the main thread is the client world's stream (the identification
        // draws); `GC+0x800704` (build mode) is only written by the ctor, 0.
        let mut rng: MsvcRand = std::mem::take(&mut self.shared.write_world().rng);
        let actions = self.ui.on_mouse_down_with(&mut self.game, &mut rng, b as i32, over, hovered_button, hovered_equipment, shift, false);
        self.shared.write_world().rng = rng;
        self.write_back_from_view();
        for a in actions {
            self.apply_action(a);
        }
    }

    /// Slot 8 `onMouseUp` 0x0047ddd0: the engine's release (0x0047ddf6..0x0047de16), then the
    /// UI part (`GameUi::on_mouse_up`: the identification and adaption "Goodbye!", the map
    /// screen's release 0x0047df01 (teleporter pick and teleport, `map_screen::on_release`),
    /// the system menu at 0x0047e00e).
    pub fn on_mouse_up(&mut self, b: MouseButton) {
        self.vk.mouse_up(b);
        match b {
            MouseButton::Left => self.ui.gui.inject_left_up(),
            MouseButton::Right => self.ui.gui.inject_right_up(),
            MouseButton::Middle => {}
        }
        self.mirror_to_view();
        let teleport_map = self.teleport_map;
        let pick = &mut self.teleport_pick;
        let mut map_release = |game: &mut GameView| {
            // 0x0047df01..0x0047e008: the map screen's release; a teleport writes the player's
            // position (`creature+0x10`, 0x0042c5b0 under the world lock), which the write-back
            // below stores into the entity.
            if let Some(p) = crate::map_screen::on_release(b as i32, &mut game.map_open, teleport_map, pick) {
                for (i, v) in p.iter().enumerate() {
                    game.player.0[i * 8..i * 8 + 8].copy_from_slice(&v.to_le_bytes());
                }
            }
        };
        let actions = self.ui.on_mouse_up(&mut self.game, b as i32, &mut map_release);
        self.write_back_from_view();
        for a in actions {
            self.apply_action(a);
        }
    }

    /// Slot 9 `onMouseWheel` 0x0047ef40 (`delta` is the sign of the wheel delta).
    pub fn on_mouse_wheel(&mut self, delta: i32) {
        self.ui.gui.wheel += delta;
        let map_open = self.game.map_open;
        self.player.camera.on_mouse_wheel(map_open, delta);
    }

    /// The local player's appearance from [`GameView::style`]: the ctor's `0x0043f7c0` at
    /// 0x004621ea (over the creature's own defaults) and the style widget's apply 0x0042c080
    /// (`reset`: a fresh `Appearance` 0x00428750 copied in by 0x00459800 first).
    fn rebuild_player_appearance(&mut self, reset: bool) {
        let style = style_bytes(&self.game.style);
        let mut rng: MsvcRand = std::mem::take(&mut self.shared.write_world().rng);
        {
            let mut nw = self.net.world.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(e) = nw.entities.get_mut(&self.player.id) {
                if reset {
                    let n = cw_world::appearance::Appearance::SIZE;
                    e.0[0x68..0x68 + n].copy_from_slice(&cw_world::appearance::Appearance::NEW.to_bytes());
                }
                appearance_from_style(e, &style, &mut rng);
                self.game.player = e.clone();
            }
        }
        self.shared.write_world().rng = rng;
        self.applied_style = self.game.style;
    }

    /// `Node::isAnimating` 0x006364f0 on a member node, over the `.plx` scene it came from.
    fn plx_animating(&self, n: Option<cw_ui::widget::NodeId>) -> bool {
        n.is_some_and(|n| self.plx.loaded.iter().any(|l| l.scene.is_animating(&self.ui.gui, n)))
    }

    fn camera_options(&self) -> CameraOptions {
        CameraOptions { camera_speed: self.options.camera_speed, camera_smoothness: self.options.camera_smoothness, invert_y: self.options.invert_y != 0 }
    }

    fn ui_state(&self) -> UiState {
        let m = &self.ui.m;
        let v = |n| self.ui.visible(n);
        UiState {
            text_focus: self.ui.gui.has_text_focus().is_some(),
            cursor_free: self.is_cursor_free(),
            movement_panel_open: v(m.start_root) || v(m.char_create_root) || v(m.world_create_root) || v(m.char_select_root) || v(m.server_root) || v(m.world_select_root),
            camera_panel_open: v(m.start_root) || v(m.char_select_root) || v(m.server_root) || v(m.world_select_root),
            camera_low_panel_open: v(m.char_create_root) || v(m.world_create_root),
            chat_active: self.ui.chat.input_active,
            widget_hovered: self.ui.gui.hovered.is_some(),
            inventory_open: v(m.inventory_panel),
            map_open: self.game.map_open,
            click_to_walk: false,
        }
    }

    fn listener(&self) -> Listener {
        let mut vp = [0f32; 16];
        vp.copy_from_slice(&self.player.camera.view.0);
        Listener { pos: self.player.camera.eye, view_proj: vp, sound_volume: self.options.sound_volume }
    }

    // -----------------------------------------------------------------------------------------
    // UI actions

    /// Performs what the UI asked for (the GameController handlers behind the connections and
    /// the menu callbacks).
    pub fn apply_action(&mut self, a: UiAction) {
        match a {
            UiAction::Quit => self.quit = true,
            UiAction::StartWorld { seed, name } => self.start_world(seed, &name),
            UiAction::StartWorldTimeSeed => self.start_world(net::time_get_time() as i32, ""),
            UiAction::StartMenuCamera => {
                let c = &mut self.player.camera;
                c.dist = 0.0;
                c.dist_target = 0.0;
                c.rot_target[0] = 120.0;
                c.rot_target[2] = 0.0;
            }
            UiAction::ResetCamera { yaw } => {
                // 0x00483efe / 0x0048418c / 0x00482580: `+0x1c0 = +0x1bc = 5`, `+0x1b0..+0x1b8
                // = (90, 0, yaw)`, then `+0x1a4..+0x1ac = +0x1b0..+0x1b8`: the eased angles
                // are set too (no easing from the previous screen's orbit).
                let c = &mut self.player.camera;
                c.dist_target = 5.0;
                c.dist = 5.0;
                c.rot_target = [90.0, 0.0, yaw];
                c.rot = c.rot_target;
            }
            UiAction::OrbitCamera => {
                let c = &mut self.player.camera;
                c.dist_target = 10.0;
                c.rot_target[0] = 100.0;
                c.rot_target[2] += self.frame_ms as f32 * 0.005;
            }
            UiAction::Connect { address } => self.connect(&address),
            UiAction::Disconnect => self.disconnect(),
            UiAction::SendChat { text, local_echo } => {
                // 0x0047e61a: connected, the input goes to the send queue `+0x1000e60`;
                // otherwise (0x0047e652..0x0047e6b0) it is pushed onto the received chat
                // `+0x1000e58` under the player's id (`creature+8`), printed with the name like
                // any received line.
                let t: Vec<u16> = text.encode_utf16().collect();
                let mut nw = self.net.world.lock().unwrap_or_else(|e| e.into_inner());
                if !local_echo && self.net.socket_open.load(Ordering::SeqCst) {
                    nw.queue_chat(t);
                } else {
                    nw.chat_in.push_back(net::ChatLine { sender: self.player.id, text: t });
                }
            }
            UiAction::Print { text, color } => {
                let c = [(color[0] * 255.0) as u8, (color[1] * 255.0) as u8, (color[2] * 255.0) as u8];
                self.print(&text, c);
            }
            UiAction::ApplyOptions(o) => {
                // `applyOptions` 0x0046f390: copy, save options.cfg (music volume: no music).
                self.options = o;
                self.game.options = o;
                self.shared.render_distance.store(o.render_distance, Ordering::Relaxed);
                if let Err(e) = o.save(self.game_dir.join(OPTIONS_FILE)) {
                    eprintln!("cw-client: options.cfg: {e}");
                }
            }
            UiAction::PlaySound { id, volume, pitch } => {
                let l = self.listener();
                self.audio.play_sound(&l, id, l.pos, volume, pitch);
            }
            UiAction::QueuePickups { items } => {
                for it in items {
                    let mut b = [0u8; Item::SIZE];
                    let n = it.len().min(Item::SIZE);
                    b[..n].copy_from_slice(&it[..n]);
                    self.player.pending_pickups.push_back(b);
                }
                self.player.pickup_timer = 0;
            }
            UiAction::UseItem { tab, index } => {
                let id = self.player.id;
                {
                    let mut guard = self.net.world.lock().unwrap_or_else(|e| e.into_inner());
                    let nw = &mut *guard;
                    if let (Some(e), Some(st)) = (nw.entities.get_mut(&id), nw.states.get_mut(&id)) {
                        interact::use_item(&mut self.player, e, &mut st.inventory, tab as usize, index as usize);
                    }
                }
                self.mirror_to_view();
            }
            UiAction::SaveCharacter { index } => self.save_character(index),
            UiAction::LoadCharacter { index } => self.load_character(index),
            UiAction::NewCharacter => self.new_character(),
            UiAction::DeleteCharacter { index } => self.delete_character(index),
            UiAction::ReloadCharacters => self.reload_characters(),
            UiAction::SaveWorld { index } => self.save_world(index),
            UiAction::DeleteWorld { index, name } => self.delete_world(index, &name),
            UiAction::RefreshPreviews | UiAction::RebuildRecipes | UiAction::SetItemTarget { .. } => {
                self.ui.handle_own_action(&self.game, &a);
            }
            UiAction::RefreshShop => self.refresh_shop(),
            UiAction::LearnSkills => {
                // The Learn click (0x0047cb..): the cost paid from the coins and the
                // allocation written to the player; the view's coins go back to the inventory.
                self.ui.skills.learn(&mut self.game.player, &mut self.game.coins);
                self.write_back_from_view();
            }
            UiAction::ItemReceived { item } => {
                let id = self.player.id;
                let fx = {
                    let nw = self.net.world.lock().unwrap_or_else(|e| e.into_inner());
                    received::item_received(&nw.entities, self.ui.text.as_deref(), id, id, &item)
                };
                for f in fx {
                    self.apply_effect(f);
                }
            }
            UiAction::SetName { name } => {
                // `/name` (0x0047e3ec..0x0047e41e): a `strcpy` of the narrowed name into
                // `creature+0x1168` (16 bytes here; the original does not bound it).
                let b = name.as_bytes();
                let n = b.len().min(15);
                self.game.player.0[0x1158..0x1168].fill(0);
                self.game.player.0[0x1158..0x1158 + n].copy_from_slice(&b[..n]);
                self.write_back_from_view();
            }
            UiAction::SetPetName { name } => self.set_pet_name(&name),
        }
    }

    /// `/namepet` (0x0047e57b..0x0047e604): with a pet item (type 0x13) in `creature+0x1020`,
    /// under the world lock the item's spirit count (`+0x114`) becomes the name's length and
    /// spirit `k` holds `(0, 0, 0)` with the `k`-th character (narrowed) as its material.
    fn set_pet_name(&mut self, name: &str) {
        let id = self.player.id;
        let mut nw = self.net.world.lock().unwrap_or_else(|e| e.into_inner());
        let Some(e) = nw.entities.get_mut(&id) else { return };
        let o = 0x1010; // creature+0x1020
        if e.0[o] != 0x13 {
            return;
        }
        let chars: Vec<u8> = name.encode_utf16().map(|c| c as u8).collect();
        e.0[o + 0x114..o + 0x118].copy_from_slice(&(chars.len() as u32).to_le_bytes());
        for (k, &c) in chars.iter().enumerate().take(32) {
            let s = o + 0x14 + k * 8;
            e.0[s..s + 3].fill(0);
            e.0[s + 3] = c;
        }
        let copy = e.clone();
        drop(nw);
        self.game.player = copy;
        self.mirror_to_view();
    }

    // -----------------------------------------------------------------------------------------
    // The inventory mirror

    /// The view the UI reads, from the authoritative state (every frame before the UI and
    /// before the click handlers): the player's block, the equipment slots
    /// (`creature+0x418 + k * 0x118`), the bag (`creature+0x11dc` tabs, the inventory of
    /// [`CreatureState`]), the held stack (`creature+0x11e8`/`+0x11ec`, [`LocalPlayer`]) and the
    /// coins (`+0x1304`/`+0x1308`, the inventory's gold and platinum).
    pub fn mirror_to_view(&mut self) {
        let id = self.player.id;
        let nw = self.net.world.lock().unwrap_or_else(|e| e.into_inner());
        // The character sheet's guard and buffs (`CharacterWidget::update` 0x00434e30).
        self.game.sheet = ui::character_sheet::SheetState::of(nw.states.get(&id));
        if let Some(e) = nw.entities.get(&id) {
            self.game.player = e.clone();
            for (k, slot) in self.game.equipment.iter_mut().enumerate() {
                let o = EQUIPMENT_AT + k * Item::SIZE;
                slot.clear();
                slot.extend_from_slice(&e.0[o..o + Item::SIZE]);
            }
        }
        // `GC+0x800a40` / `+0x800a44`: the quick menu (the gameplay closes it, 0x00491f16).
        self.ui.flag_800a40 = self.player.quick_open;
        self.game.quick_index = self.player.quick_index;
        // The skill bar `GC+0x800814` and, per slot, the cooldown map `creature+0x139c`,
        // `skillCooldown` 0x0043e6a0 and `canUseSkill` 0x0043e5a0 (`update` 0x0048bd8c..).
        if let (Some(e), Some(st)) = (nw.entities.get(&id), nw.states.get(&id)) {
            self.game.skill_bar = self
                .player
                .skill_bar
                .iter()
                .map(|&mode| ui::skill_panel::SkillBarSlot {
                    mode,
                    cooldown: u8::try_from(mode).ok().and_then(|m| st.cooldowns.get(&m).copied()).unwrap_or(0),
                    total: cw_sim::skills::skill_cooldown(e, mode, -1),
                    usable: cw_sim::combat_ai::can_use(e, st, mode),
                })
                .collect();
            // 0x0047f030 over the World's creature map.
            self.game.trainer_nearby = ui::tooltip::class_trainer_nearby(e, nw.entities.values());
        }
        if let Some(st) = nw.states.get(&id) {
            self.game.inventory = st.inventory.pages.iter().map(|p| p.iter().map(|s| ui::ItemStack { count: s.count, item: s.item.to_bytes().to_vec() }).collect()).collect();
            self.game.coins = st.inventory.gold;
            self.game.platinum = st.inventory.f12c;
        }
        self.game.held = ui::ItemStack { count: self.player.held_count, item: self.player.held_item.to_vec() };
    }

    /// The UI's edits back into the authoritative state (after the UI frame and the click
    /// handlers): the inverse of [`Controller::mirror_to_view`].
    pub fn write_back_from_view(&mut self) {
        let id = self.player.id;
        let mut nw = self.net.world.lock().unwrap_or_else(|e| e.into_inner());
        let nw = &mut *nw;
        if let Some(e) = nw.entities.get_mut(&id) {
            *e = self.game.player.clone();
            for (k, slot) in self.game.equipment.iter().enumerate() {
                let o = EQUIPMENT_AT + k * Item::SIZE;
                let n = slot.len().min(Item::SIZE);
                e.0[o..o + n].copy_from_slice(&slot[..n]);
            }
        }
        if let Some(st) = nw.states.get_mut(&id) {
            st.inventory.pages = self.game.inventory.iter().map(|p| p.iter().map(|s| cw_world::inventory::Slot { count: s.count, item: Item::from_bytes(&item_bytes(&s.item)) }).collect()).collect();
            st.inventory.gold = self.game.coins;
            st.inventory.f12c = self.game.platinum;
        }
        self.player.held_count = self.game.held.count;
        self.player.held_item = item_bytes(&self.game.held.item);
    }

    // -----------------------------------------------------------------------------------------
    // Characters and worlds (`Save/characters.db`, `Save/worlds.db`)

    /// The local player as a character record (what 0x00487520 copies under the world lock).
    /// With a named world loaded, the player's position is first stored in the record's map
    /// under that world (0x00487576..0x004875c4).
    fn capture_record(&mut self) -> CharacterRecord {
        let loaded = self.shared.lock_world_cs().loaded.clone();
        let id = self.player.id;
        let nw = self.net.world.lock().unwrap_or_else(|e| e.into_inner());
        let e = nw.entities.get(&id).cloned().unwrap_or_else(fresh_player_entity);
        let st = nw.states.get(&id).cloned().unwrap_or_default();
        drop(nw);
        let pos = scene::entity_pos(&e);
        let mut positions = self.shared.positions.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((seed, name)) = &loaded
            && !name.is_empty()
        {
            positions.insert((*seed, name.as_bytes().to_vec()), pos);
        }
        let i32_at = |o: usize| i32::from_le_bytes(e.0[o..o + 4].try_into().unwrap());
        let mut r = CharacterRecord { version: persist::VERSION, position: pos, ..CharacterRecord::default() };
        r.rotation.copy_from_slice(&e.0[0x18..0x24]);
        r.hp = f32::from_le_bytes(e.0[0x15c..0x160].try_into().unwrap());
        r.xp = i32_at(0x184);
        r.level = i32_at(0x180);
        r.class = e.0[0x130];
        r.specialization = e.0[0x131];
        r.ride_stamina = st.riding.ride_stamina;
        r.f119c = self.player_f119c;
        for k in 0..persist::ITEM_SLOTS {
            let o = persist::ITEM_SLOTS_AT + k * persist::ITEM;
            r.items[k].copy_from_slice(&e.0[o..o + persist::ITEM]);
        }
        let n = &e.0[0x1158..0x1168];
        r.name = n[..n.iter().position(|&c| c == 0).unwrap_or(n.len())].to_vec();
        r.style = style_bytes(&self.game.style);
        r.tabs = st.inventory.pages.iter().map(|p| p.iter().map(stack_bytes).collect()).collect();
        r.coins = st.inventory.gold;
        r.platinum = st.inventory.f12c;
        r.formulas = self.game.formulas.iter().map(|f| item_bytes(f)).collect();
        r.positions = positions.clone();
        r.world_seed = self.game.current_world.seed;
        r.world_name = self.game.current_world.name.as_bytes().to_vec();
        r.mana_cubes = i32_at(0x1154);
        for k in 0..11 {
            r.skills[k] = i32_at(0x1128 + 4 * k);
        }
        r
    }

    /// A record into the local player (0x0044be40 through `loadCharacter` 0x004806c0): the
    /// creature reset (0x00446330, the default entity 0x0043c100, hostility 0), the record's
    /// fields written, the position kept from before and copied to the render position, the
    /// level and XP shown (`creature+0x1d3c`/`+0x1d40`). The entity type and appearance are rebuilt
    /// from the style record (0x0043f7c0).
    fn apply_record(&mut self, r: &CharacterRecord) {
        let id = self.player.id;
        // The CRT `rand()` (the world's stream), taken before lock A (the lock order of `simulate`).
        let mut rng: MsvcRand = std::mem::take(&mut self.shared.write_world().rng);
        let net = Arc::clone(&self.net);
        let mut guard = net.world.lock().unwrap_or_else(|e| e.into_inner());
        let nw = &mut *guard;
        let old_pos = nw.entities.get(&id).map_or(r.position, scene::entity_pos);
        let mut e = fresh_player_entity();
        e.0[0x18..0x24].copy_from_slice(&r.rotation);
        e.0[0x15c..0x160].copy_from_slice(&r.hp.to_le_bytes());
        e.0[0x184..0x188].copy_from_slice(&r.xp.to_le_bytes());
        e.0[0x180..0x184].copy_from_slice(&r.level.to_le_bytes());
        e.0[0x130] = r.class;
        e.0[0x131] = r.specialization;
        for k in 0..persist::ITEM_SLOTS {
            let o = persist::ITEM_SLOTS_AT + k * persist::ITEM;
            e.0[o..o + persist::ITEM].copy_from_slice(&r.items[k]);
        }
        e.0[0x1158..0x1168].fill(0);
        let n = r.name.len().min(15);
        e.0[0x1158..0x1158 + n].copy_from_slice(&r.name[..n]);
        e.0[0x1154..0x1158].copy_from_slice(&r.mana_cubes.to_le_bytes());
        for k in 0..11 {
            e.0[0x1128 + 4 * k..0x112c + 4 * k].copy_from_slice(&r.skills[k].to_le_bytes());
        }
        // 0x0043f7c0(creature+0x64, creature+0x78, style record): the entity type from the
        // race and gender, the appearance from the record's variants and hair colour.
        appearance_from_style(&mut e, &r.style, &mut rng);
        for i in 0..3 {
            e.0[i * 8..i * 8 + 8].copy_from_slice(&old_pos[i].to_le_bytes());
        }
        let mut st = fresh_player_state();
        st.riding.ride_stamina = r.ride_stamina;
        st.riding.render_pos = old_pos;
        st.inventory.pages = r.tabs.iter().map(|t| t.iter().map(|b| cw_world::inventory::Slot { count: i32::from_le_bytes(b[0..4].try_into().unwrap()), item: Item::from_bytes(&b[4..]) }).collect()).collect();
        st.inventory.gold = r.coins;
        st.inventory.f12c = r.platinum;
        nw.entities.insert(id, e);
        nw.states.insert(id, st);
        drop(guard);
        self.shared.write_world().rng = rng;
        self.player_f119c = r.f119c;
        self.player.held_count = 0;
        self.player.held_item = [0; Item::SIZE];
        self.player.shown_level = r.level;
        self.player.shown_xp = r.xp;
        self.game.style = style_from_bytes(&r.style);
        self.applied_style = self.game.style;
        self.game.formulas = r.formulas.iter().map(|f| f.to_vec()).collect();
        self.game.current_world = ui::WorldEntry { name: String::from_utf8_lossy(&r.world_name).into_owned(), seed: r.world_seed, explored: 0 };
        *self.shared.positions.lock().unwrap_or_else(|e| e.into_inner()) = r.positions.clone();
        self.mirror_to_view();
    }

    /// `saveCharacter(i, player)` 0x00487520: the player captured, kept as the saved
    /// character `i` and written to `Save/characters.db` under `i` (compressed); the count
    /// (`"num"`) follows the list (the create handler 0x004821a0 writes it).
    fn save_character(&mut self, index: i32) {
        if index < 0 {
            return;
        }
        let rec = self.capture_record();
        let i = index as usize;
        if i < self.characters.len() {
            self.characters[i] = rec.clone();
        } else if i == self.characters.len() {
            self.characters.push(rec.clone());
        } else {
            return;
        }
        if let Some(db) = &self.char_db {
            if db.count() != self.characters.len() as i32 {
                db.set_count(self.characters.len() as i32);
            }
            db.put(index, &rec.to_blob());
        }
        self.refresh_character_entries();
    }

    /// `loadCharacter(i, player)` 0x004806c0 on the local player: the record read from the
    /// database (the in-memory one when the database has none).
    fn load_character(&mut self, index: i32) {
        if index < 0 {
            return;
        }
        let rec = self.char_db.as_ref().and_then(|db| db.get(index)).map(|b| CharacterRecord::from_blob(&b)).or_else(|| self.characters.get(index as usize).cloned());
        if let Some(r) = rec {
            self.apply_record(&r);
        }
    }

    /// "New character" (0x00483e70 with 0x00446330): a fresh player (the constructor's
    /// entity), an empty style record, no formulas, no saved positions.
    fn new_character(&mut self) {
        let id = self.player.id;
        {
            let mut nw = self.net.world.lock().unwrap_or_else(|e| e.into_inner());
            let pos = nw.entities.get(&id).map_or([0; 3], scene::entity_pos);
            let mut e = fresh_player_entity();
            for i in 0..3 {
                e.0[i * 8..i * 8 + 8].copy_from_slice(&pos[i].to_le_bytes());
            }
            nw.entities.insert(id, e);
            nw.states.insert(id, fresh_player_state());
        }
        self.player_f119c = 0;
        self.player.held_count = 0;
        self.player.held_item = [0; Item::SIZE];
        self.player.shown_level = 1;
        self.player.shown_xp = 0;
        self.game.style = ui::Style::default();
        self.game.formulas.clear();
        self.shared.positions.lock().unwrap_or_else(|e| e.into_inner()).clear();
        self.mirror_to_view();
        // 0x00483e70 resets the creature before the style widget's reset and apply
        // (0x0042bd90, 0x0042c080): the UI ran those before this action, so apply again
        // over the fresh creature (its appearance and kit follow after the UI frame).
        self.ui.char_style.apply(&mut self.game);
        self.write_back_from_view();
    }

    /// `deleteCharacter` 0x004816f0 (the UI removed the entry): the creature dropped, `"num"`
    /// rewritten and the later records moved down a key.
    fn delete_character(&mut self, index: i32) {
        if index < 0 || index as usize >= self.characters.len() {
            return;
        }
        self.characters.remove(index as usize);
        if let Some(db) = &self.char_db {
            db.remove_shift(index, self.characters.len() as i32);
        }
        self.refresh_character_entries();
    }

    /// The system menu's "Start Menu" (0x0047e0d0): every saved character reloaded from the
    /// database (0x004806c0 per index).
    fn reload_characters(&mut self) {
        if let Some(db) = &self.char_db {
            for (i, c) in self.characters.iter_mut().enumerate() {
                if let Some(b) = db.get(i as i32) {
                    *c = CharacterRecord::from_blob(&b);
                }
            }
        }
        self.refresh_character_entries();
    }

    /// `saveWorld(i, worlds[i], lock)` 0x004878a0: when the entry is the loaded world
    /// (`0x0040c520` on the name, the seed against `GC+0x800448`) and the map has a model for
    /// the player's zone, that model becomes the preview; then the record under `i`. An index
    /// past the list (the delete handler's quirk can pass one) writes nothing.
    fn save_world(&mut self, index: i32) {
        if index < 0 || index as usize >= self.game.worlds.len() {
            return;
        }
        let i = index as usize;
        if self.world_previews.len() < self.game.worlds.len() {
            self.world_previews.resize(self.game.worlds.len(), None);
        }
        let w = self.game.worlds[i].clone();
        let loaded = self.shared.lock_world_cs().loaded.clone();
        if loaded.as_ref().is_some_and(|(s, n)| *s == w.seed && *n == w.name) {
            let p = {
                let nw = self.net.world.lock().unwrap_or_else(|e| e.into_inner());
                nw.entities.get(&self.player.id).map_or([0; 3], scene::entity_pos)
            };
            let (zx, zy) = (((p[0] / 65536) as i32) / 256, ((p[1] / 65536) as i32) / 256);
            let t = self.shared.tiles.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(tile) = t.entry(zx, zy).and_then(|e| e.tile.as_ref()) {
                self.world_previews[i] = Some((tile.size, tile.voxels.iter().flatten().copied().collect()));
            }
        }
        let rec = WorldRecord { name: w.name.as_bytes().to_vec(), seed: w.seed, explored: w.explored, preview: self.world_previews[i].clone() };
        if let Some(db) = &self.world_db {
            if db.count() != self.game.worlds.len() as i32 {
                db.set_count(self.game.worlds.len() as i32);
            }
            db.put(index, &rec.to_blob());
        }
    }

    /// `deleteWorld` 0x00481d30 (the UI removed the entry and asks for the rewrites):
    /// `Save\world_<name>.db` and `Save\map_<name>.db` deleted (`DeleteFileA`), `"num"`
    /// rewritten.
    fn delete_world(&mut self, index: i32, name: &str) {
        let _ = std::fs::remove_file(persist::world_db_path(&self.game_dir, name));
        let _ = std::fs::remove_file(persist::map_db_path(&self.game_dir, name));
        if index >= 0 && (index as usize) < self.world_previews.len() {
            self.world_previews.remove(index as usize);
        }
        if let Some(db) = &self.world_db {
            db.set_count(self.game.worlds.len() as i32);
        }
    }

    /// `0x004a2300`: the shop's pages from the vendor the player talks to
    /// (`LocalPlayer::menu_creature`; its held stack is not modelled) or the buy-back list.
    fn refresh_shop(&mut self) {
        let vendor = self.player.menu_creature;
        let nw = self.net.world.lock().unwrap_or_else(|e| e.into_inner());
        let inv = nw.states.get(&vendor).map(|s| s.inventory.clone());
        drop(nw);
        let pages: Option<Vec<Vec<ui::ItemStack>>> = inv.as_ref().map(|i| i.pages.iter().map(|p| p.iter().map(|s| ui::ItemStack { count: s.count, item: s.item.to_bytes().to_vec() }).collect()).collect());
        let held = ui::ItemStack::empty();
        let view = match (&pages, &inv) {
            (Some(p), Some(i)) => Some(ui::inventory_widget::VendorView { pages: p, held: &held, coins: i.gold, platinum: i.f12c }),
            _ => None,
        };
        // 0x004a2300 reads the shop widget's current tab (`+0x1b4`), not its selection.
        let tab = self.ui.inv.shop.tab;
        self.ui.inv.shop_data.refresh(tab, view);
    }

    /// What a received record asked for (`received::Effect`).
    fn apply_effect(&mut self, fx: received::Effect) {
        match fx {
            received::Effect::Print { text, color } => self.print(&text, color),
            received::Effect::Sound { id } => {
                let l = self.listener();
                self.audio.play_sound(&l, id, l.pos, 1.0, 1.0);
            }
            received::Effect::RebuildRecipes => self.ui.handle_own_action(&self.game, &UiAction::RebuildRecipes),
            received::Effect::RefreshBag => self.mirror_to_view(),
            // 0x0049071c: a mission's creature speaks, `0x004882e0(creature, 1)`.
            received::Effect::Talk { id } => self.creature_speech(id, true),
            // The `manacubes` animation and the quest tag's "shine" are drawn by the GUI pass
            // (not produced yet).
            received::Effect::ManaCubeAnimation | received::Effect::QuestComplete { .. } => {}
        }
    }

    /// The chat widget's `print` 0x0043a500 (width 400, the ChatWidget's; text measured with
    /// the chat font, 0x0065e720).
    fn print(&mut self, text: &str, color: [u8; 3]) {
        let ui::GameUi { chat, fonts, .. } = &mut self.ui;
        chat.print(text, color, 400.0, &|t| fonts.measure_chat(t));
    }

    /// `startWorld(seed, name)` 0x0046f620: retarget the world under locks A and B, write
    /// `(seed, name)` into the local player's record (`*(player+0x1d28)+0x24/+0x28`), delete
    /// every creature but the local player. The zone thread does the switch.
    pub fn start_world(&mut self, seed: i32, name: &str) {
        {
            let mut nw = self.net.world.lock().unwrap_or_else(|e| e.into_inner());
            let me = nw.local_player;
            let others: Vec<i64> = nw.entities.keys().copied().filter(|k| *k != me).collect();
            for k in others {
                nw.entities.remove(&k);
                nw.states.remove(&k);
                nw.render_snapshots.remove(&k);
            }
            nw.world_seed = seed as u32;
            nw.world_name = name.to_string();
        }
        {
            let mut cs = self.shared.lock_world_cs();
            cs.requested_seed = seed;
            cs.requested_name = name.to_string();
        }
        self.game.current_world = ui::WorldEntry { name: name.to_string(), seed, explored: 0 };
    }

    /// The join (0x00470402..0x00470408): the recreated player becomes the world's local
    /// player (`world+0xb8`).
    fn set_local_player(&mut self, id: i64) {
        self.player.id = id;
        self.shared.write_world().local_player = Some(id);
    }

    /// `connect` 0x0046fc50 (`/connect`, the "Connect" button 0x004815e0).
    pub fn connect(&mut self, host: &str) {
        self.disconnect();
        self.server_address = host.to_string();
        // 0x0046fca7: `world+0xb4 = 1` before the socket is made; every failure clears it.
        self.shared.write_world().is_client = true;
        match net::connect(host, &self.net) {
            Ok(c) => {
                self.session = Some(c);
                let id = self.net.world.lock().unwrap_or_else(|e| e.into_inner()).local_player;
                self.set_local_player(id);
                self.print("Connected.\n", [255, 127, 51]);
            }
            Err(e) => {
                self.shared.write_world().is_client = false;
                if let Some(m) = e.message() {
                    self.print(&m, [255, 127, 51]);
                }
            }
        }
    }

    /// `disconnect` 0x004719f0: the threads stopped, then under the world lock
    /// `world+0xb4 = 0` (0x00471aad) and "Disconnected." in (1, 0.5, 0.2), then
    /// `startWorld` of the loaded world (0x00471b35: `GC+0x800448`, `GC+0x378`), which drops
    /// every other creature. `world+0xb8` keeps the player the join made.
    pub fn disconnect(&mut self) {
        if let Some(c) = self.session.take() {
            c.disconnect(&self.net);
            self.shared.write_world().is_client = false;
            self.print("Disconnected.\n", [255, 127, 51]);
            let loaded = self.shared.lock_world_cs().loaded.clone();
            if let Some((seed, name)) = loaded {
                self.start_world(seed, &name);
            }
        }
    }

    // -----------------------------------------------------------------------------------------
    // update 0x00488ee0

    /// Slot 10 `update(dt)` 0x00488ee0.
    pub fn update(&mut self, dt: i32) {
        // 0x0048ce42: `+0x8006e8 = dt`.
        self.frame_ms = dt;
        let head = profile::scope(Phase::UpdateHead);
        self.bytes = ControllerBytes::from_input(&self.input);
        self.finish_world_loads();
        // 0x00489020..0x00489372: the fog distance.
        self.fog_step(dt);
        drop(head);
        // 0x00489372..0x00489ca0 and the widget updates: the UI frame.
        let ui = profile::scope(Phase::Ui);
        self.ui_frame(dt);
        // Packet 15 asked for another world (the receive thread's `startWorld` call with
        // `online_<seed>`, 0x0046b907..0x0046ba60, then the GUI root shown and the three
        // select/create screens hidden).
        let request = self.net.world.lock().unwrap_or_else(|e| e.into_inner()).world_request.take();
        if let Some(r) = request
            && self.session.is_some()
        {
            self.start_world(r.seed as i32, &r.name);
            let m = self.ui.m.clone();
            self.ui.set_visible(m.gui_root, true);
            for n in [m.char_create_root, m.char_select_root, m.world_select_root] {
                self.ui.set_visible(n, false);
            }
        }
        drop(ui);
        // The world tick and everything under lock A.
        let sounds = self.simulate(dt);
        let _send = profile::scope(Phase::Send);
        for s in sounds {
            let l = self.listener();
            // 0x00495630..: the camera shakes on sounds 0x51 and 0x52 (`audio::play_record`).
            if self.audio.play_record(&l, &s) {
                self.player.camera.shake = 1.0;
            }
        }
        for fx in std::mem::take(&mut self.effects) {
            self.apply_effect(fx);
        }
        self.player_events();
        // The mesher's character save (0x004690a0, end of each pass): `60000 < now - last`
        // (`timeGetTime`, `last` = 0x0076b078, zero at start, so the first save follows the
        // first named world at once) with the world's name not empty (`GC+0x388`, the length
        // of `GC+0x378`): `0x00487520(GC+0x800a0c, player)`, `last = now`. Kept on the main
        // thread (the port's mesher does not hold the character database; the save runs
        // between two frames instead of between two mesh passes, 5 ms apart).
        let now = net::time_get_time();
        if (now.wrapping_sub(self.last_character_save) as i32) > CHARACTER_SAVE_MS {
            let named = self.shared.lock_world_cs().loaded.as_ref().is_some_and(|l| !l.1.is_empty());
            if named {
                self.save_character(self.game.selected_character);
                self.last_character_save = now;
            }
        }
        // 0x0049cee0..0x0049d110.
        self.publish_thread_inputs();
        self.stats(dt);
    }

    /// `CW_CLIENT_STATS=1`: a line every five seconds (the port's own diagnostics).
    fn stats(&mut self, dt: i32) {
        if !profile::enabled() {
            return;
        }
        let before = self.stats_ms;
        self.stats_ms = self.stats_ms.wrapping_add(dt);
        if before / 5000 == self.stats_ms / 5000 {
            return;
        }
        if std::env::var_os("CW_DUMP_GUI").is_some() {
            // The visible nodes with a shape or widget under the engine root: name path and
            // screen position (the port's own diagnostics).
            let g = &self.ui.gui;
            let mut stack: Vec<(cw_ui::widget::NodeId, String)> = g.root.map(|r| (r, String::new())).into_iter().collect();
            while let Some((n, path)) = stack.pop() {
                let node = &g.nodes[n];
                if !node.visible {
                    continue;
                }
                let p = format!("{path}/{}", node.name);
                if node.shape.is_some() || node.widget.is_some() {
                    let o = g.node_world(n).transform_point2(glam::Vec2::ZERO);
                    eprintln!("gui: {p} at ({:.0}, {:.0})", o.x, o.y);
                }
                for &c in node.children.iter().rev() {
                    stack.push((c, p.clone()));
                }
            }
        }
        let zones = self.shared.read_world().loaded_zones().len();
        let (meshed, verts) = {
            let r = self.shared.lock_mesh_cs();
            (r.records.iter().filter(|c| c.build.is_some()).count(), r.records.iter().map(|c| c.vertex_count()).sum::<usize>())
        };
        let (creatures, pos) = {
            let nw = self.net.world.lock().unwrap_or_else(|e| e.into_inner());
            (nw.entities.len(), nw.entities.get(&self.player.id).map(scene::entity_pos).unwrap_or([0; 3]))
        };
        eprintln!(
            "cw-client: {} zones, {} chunks meshed ({} vertices), {} creatures, player {:.1},{:.1},{:.1}, fog {:.1}, connected {}",
            zones,
            meshed,
            verts,
            creatures,
            pos[0] as f64 / 65536.0,
            pos[1] as f64 / 65536.0,
            pos[2] as f64 / 65536.0,
            self.fog_distance,
            self.net.connected.load(Ordering::SeqCst)
        );
    }

    /// The zone thread's world loads, finished on the main thread: the map database
    /// (`WorldMap::load 0x005fbc90`), the old one saved first.
    fn finish_world_loads(&mut self) {
        let loaded: Vec<(i32, String)> = std::mem::take(&mut *self.shared.loaded_events.lock().unwrap_or_else(|e| e.into_inner()));
        for (seed, name) in loaded {
            if let Some(old) = self.map_world.take()
                && let Err(e) = singleplayer::save_map(&self.game_dir, &old, &self.shared.tiles, None, &mut self.map_blobs)
            {
                eprintln!("cw-client: map save: {e}");
            }
            singleplayer::load_map(&self.game_dir, &name, &self.shared.tiles, &mut self.map_blobs);
            self.map_world = Some(name.clone());
            self.projectiles.clear();
            self.particles = ParticleSystem::new();
            self.items8.clear();
            self.clock = cw_sim::day::Clock::default();
            // The zone thread's world list step (0x0046b0..0x0046b1d4): a named world is
            // selected in the saved list, or appended (a server's world), saved with the new
            // count, and the world previews rebuilt.
            if !name.is_empty() {
                match self.game.worlds.iter().position(|w| w.name == name && w.seed == seed) {
                    Some(i) => self.game.selected_world = i as i32,
                    None => {
                        self.game.worlds.push(ui::WorldEntry { name: name.clone(), seed, explored: 0 });
                        let i = self.game.worlds.len() as i32 - 1;
                        self.game.selected_world = i;
                        self.save_world(i);
                        self.apply_action(UiAction::RefreshPreviews);
                    }
                }
            }
        }
    }

    /// `update` 0x00489020..0x00489372: over the ring window, the squared horizontal distance
    /// from the camera (`+0x140`) to the middle of every chunk without buffers (or holding other
    /// coordinates), starting from `+0x1d0²`; `d = sqrt(min) - 22.4` eased into `+0x1d4` with
    /// `lerpRepeat(dt, 0.001)`; below 1e-4, or with the world not loaded, `+0x1d4 = 1e-4`.
    pub fn fog_step(&mut self, dt: i32) {
        let mut best = self.view_distance * self.view_distance;
        {
            let ring = self.shared.lock_mesh_cs();
            let w = ring.window;
            for x in w.origin[0]..w.origin[0] + w.size {
                for y in w.origin[1]..w.origin[1] + w.size {
                    if x < 0 || y < 0 || x >= 0x80000 || y >= 0x80000 {
                        continue;
                    }
                    let r = &ring.records[ring.index(x, y)];
                    if r.build.is_some() && r.coords == Some([x, y]) {
                        continue;
                    }
                    // `((x << 16) + 0x8000) << 5`: the chunk middle in fixed point.
                    let cx = ((i64::from(x) << 16) + 0x8000) << 5;
                    let cy = ((i64::from(y) << 16) + 0x8000) << 5;
                    let k = 1.525_878_9e-5f32;
                    let dx = (cx.wrapping_sub(self.player.camera.eye[0]) as f32) * k;
                    let dy = (cy.wrapping_sub(self.player.camera.eye[1]) as f32) * k;
                    let d2 = dy * dy + dx * dx;
                    if best > d2 {
                        best = d2;
                    }
                }
            }
        }
        let d = (f64::from(best).sqrt() as f32) - 22.4f32;
        let f = player::lerp_factor(dt, 0.001);
        self.fog_distance = (1.0 - f) * self.fog_distance + f * d;
        let matches = self.shared.lock_world_cs().matches();
        if 0.0001f32 > self.fog_distance || !matches {
            self.fog_distance = 0.0001;
        }
    }

    /// The UI half of `update`: the view of the client state, the screen rules and the widget
    /// frames, the events of the widget tree routed to the handlers.
    fn ui_frame(&mut self, dt: i32) {
        let id = self.player.id;
        // 0x0048b627..0x0048b802: the map pan. Map open with the middle button (`GC+0xa`)
        // held: the engine cursor's last motion (`Engine+0xd4 - +0xdc`, `+0xd8 - +0xe0`; the
        // app calls slot 6 with the absolute cursor every frame while it is free, and the map
        // frees it) scaled by `4 / zoom` and turned by the camera yaw `GC+0x1ac`; map closed,
        // zero. The original runs this after the screen rules; the port runs it before the UI
        // frame so the overlay labels (which the original computes in `render`) see the new
        // pan. The M key and the menu button toggle the map in the input slots, before
        // `update`, so the map state is the same either way.
        {
            let d = self.ui.gui.cursor - self.ui.gui.prev_cursor;
            let (yaw, zoom) = (self.player.camera.rot[2], self.player.camera.map_zoom);
            crate::map_screen::update_pan(&mut self.map_pan, self.game.map_open, self.bytes.middle_mouse(), yaw, zoom, [d.x, d.y]);
        }
        let teleport_selected = self.teleport_pick.selected;
        self.mirror_to_view();
        let stamina = {
            let nw = self.net.world.lock().unwrap_or_else(|e| e.into_inner());
            nw.states.get(&id).map_or(1.0, |s| s.stamina)
        };
        self.game.connected = self.net.connected.load(Ordering::SeqCst);
        self.game.socket_open = self.net.socket_open.load(Ordering::SeqCst);
        self.game.load_distance = self.fog_distance;
        self.game.world_pending = !self.shared.lock_world_cs().matches();
        self.game.engine_time_ms = self.game.engine_time_ms.wrapping_add(dt);
        self.ui.gui.frame_ms = dt;
        // The CRT `rand()` of the main thread is the client world's stream.
        let world_name = self.shared.lock_world_cs().loaded.as_ref().map(|l| l.1.clone()).unwrap_or_default();
        // The world parts of the UI frame read the client world, the creature map and the map
        // tiles (lock order as `simulate`: the world, then lock A, then the tiles).
        let shared = Arc::clone(&self.shared);
        let net = Arc::clone(&self.net);
        let mut world_guard = shared.write_world();
        let mut rng: MsvcRand = std::mem::take(&mut world_guard.rng);
        let world: &World = &world_guard;
        let nw = net.world.lock().unwrap_or_else(|e| e.into_inner());
        let tiles = shared.lock_tiles();
        // The dictionary of the landscape names (0x004e5c10 / 0x004e5320, `hud::landscape_names`).
        let text_db = self.ui.text.clone();
        let out = {
            let screen = [self.screen[0], self.screen[1]];
            let player_e = nw.entities.get(&id).cloned().unwrap_or_else(fresh_player_entity);
            let player_pos = scene::entity_pos(&player_e);
            let st = nw.states.get(&id);
            let (guard, haste) = cw_sim::util::guard_haste(&nw.states, id);
            // 0x004911e3: the pet `World::findEntity(creature+0x11c8)`.
            let pet = st.map(|s| s.pet).filter(|p| *p != 0).and_then(|p| nw.entities.get(&p));
            // 0x0049198b..0x00491f16: the other creatures of hostility 0 (players), map order.
            let party: Vec<&EntityData> = nw.entities.iter().filter(|(k, e)| **k != id && e.0[0x50] == 0).map(|(_, e)| e).collect();
            let manacube_animating = self.plx_animating(self.ui.m.manacube_bar);
            let hud = ui::hud::HudInputs {
                player: &player_e,
                stamina,
                guard,
                riding_stamina: st.map_or(0.0, |s| s.riding.ride_stamina),
                haste,
                pet,
                party: &party,
                width: screen[0],
                height: screen[1],
                manacube_animating,
                type_name: None,
                world: Some(world),
                text: text_db.as_deref(),
            };
            // The crosshair: the aimed creature `GC+0x800a70` and static `GC+0x800a84`.
            let aimed_creature = (self.player.aimed_creature != 0).then(|| nw.entities.get(&self.player.aimed_creature)).flatten();
            let aimed_static = interact::static_at(world, self.player.aimed_static);
            let crosshair = ui::CrosshairInputs { shift: self.bytes.shift(), prompt: None, player_id: id, aimed_creature, aimed_static };
            // The quest tags: the World cell under the player (`getCell` of the block).
            let (bx, by) = ((player_pos[0] / 65536) as i32, (player_pos[1] / 65536) as i32);
            let cell = world.cell_at_block(bx, by);
            let quest_cell = cell.map(ui::target::QuestCell::from_cell);
            let quest = ui::QuestInputs {
                cell: quest_cell.as_ref(),
                falloff: cell.map_or(0.0, |c| c.falloff(player_pos[0], player_pos[1])),
                player_pos,
                objective: None,
                questtag_animating: self.plx_animating(self.ui.m.questtag),
            };
            // The nameplates (0x00490920..0x0049b18d).
            let creatures: Vec<ui::target::PlateCreature> = nw
                .entities
                .iter()
                .map(|(k, e)| {
                    let s = nw.states.get(k);
                    ui::target::PlateCreature {
                        id: *k,
                        entity: e,
                        render_pos: s.map_or(scene::entity_pos(e), |s| s.riding.render_pos),
                        step_offset: s.map_or(0.0, |s| s.step_offset),
                    }
                })
                .collect();
            let aimed = self.player.aimed_creature;
            let candidates = ui::target::plate_candidates(&creatures, player_pos, &|c| c == aimed);
            let eye = self.player.camera.eye;
            let ground_light = |x: i32, y: i32, z: i32| i32::from(interact::ground_light_at(world, [i64::from(x) << 16, i64::from(y) << 16, i64::from(z) << 16]));
            let line_of_sight = |to: [i64; 3], max: f32| cw_sim::path::line_of_sight(world, eye, to, true, max);
            let mode_of = |c: i64| nw.entities.get(&c).map(|e| e.0[0x58]);
            let creature = |c: i64| creatures.iter().find(|p| p.id == c).copied();
            let plates = ui::PlateSet {
                inputs: ui::target::PlateInputs {
                    player_id: id,
                    player: &player_e,
                    show_all: self.ui.flag_8007b4,
                    origin: self.player.camera.origin,
                    view: &self.player.camera.view_shake,
                    projection: &self.player.projection,
                    width: screen[0],
                    height: screen[1],
                    // `[esp+0xe4]` of `update` (its writer is not traced): 1.
                    light_scale: 1.0,
                    ground_light: &ground_light,
                    line_of_sight: &line_of_sight,
                    mode_of: &mode_of,
                    singular: None,
                },
                candidates,
                creature: &creature,
            };
            // The map overlay (0x004c9680) over the map's stored matrices, map open.
            let zone = |zx: i32, zy: i32| tiles.entry(zx, zy).map(ui::map_overlay::ZoneSite::from_entry);
            let cell_site = |cx: i32, cy: i32| world.cell(cx, cy).map(ui::map_overlay::CellSite::from_cell);
            let cell_falloff = |cx: i32, cy: i32, x: i64, y: i64| world.cell(cx, cy).map_or(0.0, |c| c.falloff(x, y));
            let base_height = |x: i32, y: i32| world.base_height(x, y);
            // 0x004ca085..0x004ca0b9: `0x00468840((zx, zy), GC+0x800ddc)`, the picked zone.
            let teleport_marked = |zx: i32, zy: i32| [zx, zy] == teleport_selected;
            let map = match (self.game.map_open, self.map_state.stored_view, self.map_state.stored_projection) {
                (true, Some(view), Some(projection)) => Some(ui::map_overlay::MapOverlayInputs {
                    player_pos,
                    player_level: i32::from_le_bytes(player_e.0[0x180..0x184].try_into().unwrap()),
                    centre: self.map_pan,
                    zoom: self.player.camera.map_zoom,
                    view,
                    projection,
                    width: screen[0],
                    height: screen[1],
                    cursor: self.ui.gui.cursor.to_array(),
                    teleport_mode: self.teleport_map,
                    teleport_marked: &teleport_marked,
                    zone: &zone,
                    site_name: None,
                    cell: &cell_site,
                    cell_falloff: &cell_falloff,
                    cell_name: None,
                    base_height: &base_height,
                }),
                _ => None,
            };
            // 0x0048d771..0x0048d7a9: the aimed ground item (`GC+0x800a78`, 0x0059fb90).
            let ground_item = interact::ground_item_at(world, self.player.aimed_item).map(|g| g.item.to_bytes());
            let mut inp = ui::FrameInputs::basic(&mut rng, stamina, dt, glam::IVec2::new(screen[0], screen[1]));
            inp.ground_item = ground_item.as_ref().map(|b| &b[..]);
            inp.time_ms = self.game.engine_time_ms;
            inp.left_held = self.bytes.left_mouse();
            inp.world_name = &world_name;
            inp.world = Some(world);
            inp.hud = Some(hud);
            inp.crosshair = Some(crosshair);
            inp.quest = Some(quest);
            inp.plates = Some(plates);
            inp.map = map;
            self.ui.frame(&mut self.game, &mut inp)
        };
        drop(tiles);
        drop(nw);
        world_guard.rng = rng;
        drop(world_guard);
        let mut out = out;
        let actions = std::mem::take(&mut out.actions);
        // 0x0048b638..0x0048b6c6: the "Teleport" caption over the held-stack caption, with the
        // hovered zone the previous overlay pass left (the original's overlay runs in
        // `render`, after this test).
        if let Some(t) = crate::map_screen::teleport_caption(self.game.map_open, self.teleport_map, &self.teleport_pick) {
            out.cursor_caption = t.to_string();
        }
        // `MapOverlayWidget::update` 0x004c96d7 / 0x004ca189: `GC+0x800dd4` = the teleporter
        // zone under the cursor, or (-1, -1); only while the overlay runs (map open).
        if let Some(f) = &out.map_overlay {
            self.teleport_pick.hovered = f.hovered_zone.map_or(crate::map_screen::NO_ZONE, |(x, y)| [x, y]);
        }
        // What the widgets show this frame, applied to the tree and drawn by `render`.
        ui::present_hud::animation_inputs(&mut self.ui, &self.plx);
        ui::present::apply(&mut self.ui, &self.game, &out);
        // The "hit"/"show" states and colours the HUD asked the scenes to play.
        ui::present_hud::play_scene_writes(&mut self.ui, &mut self.plx);
        self.ui_out = out;
        for a in actions {
            self.apply_action(a);
        }
        self.ui.gui.tick_all(dt);
        // `Engine::update` 0x00659fb0: every playing `.plx` attribute advances by `dt`.
        for l in self.plx.loaded.iter_mut() {
            l.scene.update(&mut self.ui.gui, dt);
        }
        let events = std::mem::take(&mut self.ui.gui.events);
        // `Node::setState` 0x00636810 from the widgets (the Button's `button:enter`,
        // `button:leave`, `button:press`... of 0x00665b00 / 0x00665b30 / 0x006659e0): the
        // states start on the `.plx` attributes of the node's subtree. Each scene plays the
        // nodes it owns (the file's nodes and their registered clones).
        for e in &events {
            if let cw_ui::widget::UiEvent::State { node, name, snap } = *e {
                for l in self.plx.loaded.iter_mut() {
                    if snap {
                        l.scene.snap_state(&mut self.ui.gui, node, name);
                    } else {
                        l.scene.set_state(&self.ui.gui, node, name, 0);
                    }
                }
            }
        }
        let actions = self.ui.apply(&events, &mut self.game);
        for a in actions {
            self.apply_action(a);
        }
        if self.game.quit {
            self.quit = true;
        }
        // The UI edits the player's block, equipment, bag, held stack and coins: write back.
        self.write_back_from_view();
        // The style widget's apply 0x0042c080 ran this frame: its tail, the appearance
        // (0x00428750 / 0x00459800 / 0x0043f7c0) then the starter kit 0x004772b0, with the
        // main thread's `rand()`. Otherwise the appearance follows a style change (a load).
        if std::mem::take(&mut self.game.style_applied) {
            self.rebuild_player_appearance(true);
            let mut rng: MsvcRand = std::mem::take(&mut self.shared.write_world().rng);
            ui::character_style::starter_kit(&mut self.game, &mut rng);
            self.shared.write_world().rng = rng;
            self.write_back_from_view();
        } else if self.game.style != self.applied_style {
            self.rebuild_player_appearance(true);
        }
        // 0x0048cc0f..0x0048cd99: the front of the received chat (`+0x1000e58`), one entry
        // per frame, printed with its sender's name (`World::findEntity` 0x0042f000 for ids
        // `>= 0`), then popped (0x00486030).
        let line = {
            let mut nw = self.net.world.lock().unwrap_or_else(|e| e.into_inner());
            nw.chat_in.pop_front().map(|l| {
                let name = (l.sender >= 0).then(|| nw.entities.get(&l.sender).map(ui::hud::name_of)).flatten();
                (name, String::from_utf16_lossy(&l.text))
            })
        };
        if let Some((name, text)) = line {
            let ui::GameUi { chat, fonts, .. } = &mut self.ui;
            chat.print_received(name.as_deref(), &text, 400.0, &|t| fonts.measure_chat(t));
        }
    }

    /// Everything of `update` between `before_tick` and the send queues, under lock A: the
    /// world tick in 20 ms slices, the received lists (`received.rs`), the gameplay ranges
    /// after them, the particles and flash lights. Returns the sounds to play (after the
    /// locks): the tick's and the received ones, then the gameplay's.
    fn simulate(&mut self, dt: i32) -> Vec<cw_net::packet::Sound> {
        let id = self.player.id;
        let socket_open = self.net.socket_open.load(Ordering::SeqCst);
        let ui = self.ui_state();
        let opts = self.camera_options();
        let shared = Arc::clone(&self.shared);
        let net = Arc::clone(&self.net);
        let tick_scope = profile::scope(Phase::Tick);
        let mut world_guard = shared.write_world();
        let world = &mut *world_guard;
        let mut nw_guard = net.world.lock().unwrap_or_else(|e| e.into_inner());
        let nw = &mut *nw_guard;
        if nw.regions_reset {
            // The join (0x0047040e..0x0047053d): every loaded region's 64 mission cells cleared
            // and every zone remeshed. The mission cells are the server's to resend (packet 12).
            nw.regions_reset = false;
            for (rx, ry) in world.loaded_regions() {
                for cx in rx * 8..rx * 8 + 8 {
                    for cy in ry * 8..ry * 8 + 8 {
                        if let Some(c) = world.cell_mut(cx, cy) {
                            c.mission = cw_world::save::MissionState::EMPTY;
                        }
                    }
                }
            }
            self.remesh_all = true;
        }
        // Packet 5 writes the day and time straight into the world (`+0x800444`/`+0x800440`);
        // between packets the tick's clock runs them on.
        if self.session.is_some() && self.applied_time != Some((nw.day, nw.time_of_day)) {
            self.applied_time = Some((nw.day, nw.time_of_day));
            world.day = nw.day as i32;
            world.time_of_day = nw.time_of_day as i32;
        }
        // 0x0048b8ab..0x0048ce2e.
        self.player.before_tick(&nw.entities, &mut nw.states, dt);
        // 0x0048cf2e: World::tick in slices, the received list NULL.
        let mut out = ServerUpdate::default();
        let interacts = std::mem::take(&mut self.pending_interacts);
        self.world_tick(world, &mut nw.entities, &mut nw.states, dt, interacts, &mut out);
        drop(tick_scope);
        let net_scope = profile::scope(Phase::Net);
        // 0x0048d5f0..0x0049081b: the received lists, applied and spliced into the output.
        let incoming = std::mem::take(&mut nw.incoming);
        let text = self.ui.text.clone();
        {
            let mut t = received::Target {
                world: &mut *world,
                entities: &mut nw.entities,
                states: &mut nw.states,
                projectiles: &mut self.projectiles,
                items8: &mut self.items8,
                dirty_zones: &mut self.dirty_zones,
                local: id,
                text: text.as_deref(),
            };
            let fx = received::apply(&mut t, incoming, &mut out);
            self.effects.extend(fx);
        }
        drop(net_scope);
        let _player_scope = profile::scope(Phase::Player);
        // 0x00491f16..0x0049ced6.
        let center_zone = {
            let p = nw.entities.get(&id).map_or([0; 3], scene::entity_pos);
            [((p[0] / 65536) as i32) / 256, ((p[1] / 65536) as i32) / 256]
        };
        let mut after = ServerUpdate::default();
        let fi = FrameInput { input: &self.bytes, ui: &ui, opts: &opts, dt, world_ms: self.world_ms, center_zone, models: &self.models };
        // 0x0049732f..0x0049735f (inside the gameplay ranges, before the R key that can open
        // the map from a bed): map closed, bed mode and the pick cleared. Nothing between
        // 0x00491f16 and here reads them.
        crate::map_screen::reset_when_closed(self.game.map_open, &mut self.teleport_map, &mut self.teleport_pick);
        self.player.after_tick(world, &mut nw.entities, &mut nw.states, &mut after, &fi);
        // 0x00498078..0x00498254: click-to-walk (never enabled in this build).
        if CLICK_TO_WALK {
            Self::click_to_walk(world, &mut nw.entities, &mut nw.states, id, dt, self.world_ms);
        }
        // 0x0049c02f..0x0049c254: the local player's hits, passives and shoots of the output
        // lists to the send queues.
        nw.queue_tick_output(socket_open, &out.hits, &out.passives, &out.shoots);
        nw.queue_tick_output(socket_open, &after.hits, &after.passives, &after.shoots);
        // `creature+0x130c`: the interactions go out as packet 6 (one per send pass) and are
        // dispatched by the next tick (0x005362ac, `client_interact` in a client world).
        for i in self.player.interact_queue.drain(..) {
            if self.session.is_some() {
                nw.queue_interact(i.clone());
            }
            self.pending_interacts.push(i);
        }
        // 0x004952a5..0x004955b1: the output particles (the tick's and the received), then the
        // gameplay's, on the global `rand()` of the client world.
        for rec in out.particles.iter().chain(after.particles.iter()) {
            self.particles.spawn_server(rec, &mut world.rng);
        }
        // 0x00495f25..0x00496329: the particles.
        self.particles.update(dt, &*world);
        // 0x004963cb..0x0049645b: flash lights.
        self.scene.flash_lights.retain_mut(|l| !l.advance(dt));
        self.world_ms = self.clock.play_ms;
        drop(nw_guard);
        drop(world_guard);
        // Lock C after lock A is released (the mesher takes C, then A).
        if self.remesh_all || !self.dirty_zones.is_empty() {
            let mut ring = shared.lock_mesh_cs();
            for r in ring.records.iter_mut() {
                if let Some([cx, cy]) = r.coords
                    && (self.remesh_all || self.dirty_zones.contains(&(cx * 32 / 256, cy * 32 / 256)))
                {
                    r.dirty = true;
                }
            }
            self.remesh_all = false;
            self.dirty_zones.clear();
        }
        let mut all = out.sounds;
        all.extend(after.sounds);
        all
    }

    /// `World::tick 0x0060c510` on the client's world, `dt` split into 20 ms slices while
    /// more than 20 remain, then the remainder, with no received list (0x0048cf20 `push 0`).
    /// Each slice is the shared tick (`cw_server::server::world_tick_with`): in singleplayer
    /// (`world+0xb4` clear) with its server passes, connected without them. After each
    /// creature's update and move the walk cycle advances (0x0061e9c6.., sections (q)..(t)).
    /// The local player's interactions go to the first slice.
    pub fn world_tick(&mut self, world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, dt: i32, interacts: Vec<cw_net::packet::Interact>, out: &mut ServerUpdate) {
        let mut remaining = dt;
        let mut first = true;
        let id = self.player.id;
        let mut interacts: Vec<(i64, cw_net::packet::Interact)> = interacts.into_iter().map(|i| (id, i)).collect();
        loop {
            let step = if remaining > TICK_MS { TICK_MS } else { remaining };
            if step <= 0 && !first {
                break;
            }
            let mut slice = ServerUpdate::default();
            let mut missions = Vec::new();
            let walk = &mut self.walk;
            let mut hook = |_w: &World, entities: &BTreeMap<i64, EntityData>, states: &BTreeMap<i64, CreatureState>, cid: i64| {
                let Some(e) = entities.get(&cid) else { return };
                let (guard, haste) = cw_sim::util::guard_haste(states, cid);
                let mount_id = states.get(&cid).map_or(0, |s| s.modes.mount);
                let mount = if mount_id != 0
                    && entities.get(&mount_id).is_some_and(|m| f32::from_le_bytes(m.0[0x15c..0x160].try_into().unwrap()) > 0.0)
                    && e.0[0x50] != 5
                {
                    walk.get(&mount_id).copied()
                } else {
                    None
                };
                let w = walk.entry(cid).or_default();
                pose::advance_walk_cycle(e, w, step, guard, haste, mount);
            };
            {
                let mut ctx = cw_server::server::TickCtx { clock: &mut self.clock, projectiles: &mut self.projectiles, verbose: false, log_regions: false };
                cw_server::server::world_tick_with(&mut ctx, world, entities, states, step, std::mem::take(&mut interacts), Vec::new(), Vec::new(), &mut missions, &mut slice, &mut hook);
            }
            // The region activations and discoveries of the player loop join the tick's
            // missions (the server broadcasts them the same way).
            out.missions.extend(missions.iter().map(|m| cw_net::packet::Mission { cell_x: m.cell_x, cell_y: m.cell_y, words: m.words, b40: m.b40, b41: m.b41, tail: m.tail }));
            net::append_update(out, slice);
            first = false;
            remaining -= step;
            if remaining <= 0 {
                break;
            }
        }
        // Creatures the tick removed (despawn, daily removal) take their render state along.
        self.walk.retain(|k, _| entities.contains_key(k));
    }

    /// Click-to-walk, 0x00498078..0x00498254 (`0x0076b040` set; never in this build): on
    /// every 32 ms boundary of the world clock the waypoints are rebuilt
    /// (`Creature::buildPath` 0x005a7c90 = `Server.exe 0x004dafe0`) and, up to ten times while
    /// they hold at most 50 blocks and the last one is not the goal's block, the search
    /// expands once (`stepPath` 0x005aaab0) and they are rebuilt; then flag 4 is cleared and
    /// the path followed (`followPath` 0x005a7eb0); when that stops with waypoints left, flag 4
    /// is set and `creature+0x160` points from the player to the middle of the last waypoint
    /// block. The movement input is skipped (`UiState::click_to_walk`).
    fn click_to_walk(world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, id: i64, dt: i32, clock_ms: i32) {
        let (Some(e), Some(st)) = (entities.get_mut(&id), states.get_mut(&id)) else { return };
        let mut path = std::mem::take(&mut st.path);
        if (clock_ms.wrapping_sub(dt)) / 32 != clock_ms / 32 {
            cw_sim::path::build_waypoints(&mut path);
            for _ in 0..10 {
                if path.waypoints.len() > 0x32 {
                    break;
                }
                let goal = [0, 1, 2].map(|i| (path.goal[i] >> 16) as i32);
                if path.waypoints.back().is_some_and(|b| *b == goal) {
                    break;
                }
                cw_sim::path::search_step(world, e, st, &mut path);
                cw_sim::path::build_waypoints(&mut path);
            }
        }
        cw_sim::update::set_flag(e, 4, false);
        let steered = cw_sim::path::follow_path(world, e, st, &mut path, dt);
        if !steered && let Some(b) = path.waypoints.back().copied() {
            cw_sim::update::set_flag(e, 4, true);
            let pos = scene::entity_pos(e);
            for i in 0..3 {
                let target = (i64::from(b[i]) << 16) + 0x8000;
                let d = target.wrapping_sub(pos[i]) as f32 / 65536.0;
                e.0[0x150 + i * 4..0x154 + i * 4].copy_from_slice(&d.to_le_bytes());
            }
        }
        st.path = path;
    }

    /// The player's events: sounds through `playSound`, the panels the movement closes, the
    /// R interactions' panels and speech bubbles.
    fn player_events(&mut self) {
        let events = std::mem::take(&mut self.player.events);
        let mut talks = Vec::new();
        let mut examines = Vec::new();
        for ev in events {
            match ev {
                Event::Sound { id, pos, volume, pitch } => {
                    let l = self.listener();
                    self.audio.play_sound(&l, id, pos, volume, pitch);
                }
                Event::CloseMovementPanels => {
                    let m = self.ui.m.clone();
                    for n in [m.inventory_panel, m.crafting_panel, m.system_panel, m.skills_panel] {
                        self.ui.set_visible(n, false);
                    }
                    self.ui.skills_open = false;
                }
                Event::OpenMapFromBed => {
                    // 0x00497424: `+0x8006e4 = +0x800de4 = 1`.
                    self.game.map_open = true;
                    self.teleport_map = true;
                }
                Event::Talk { id } => talks.push(id),
                Event::Examine { kind } => examines.push(kind),
                Event::CreatureMenu { id } => self.open_creature_menu(id),
                Event::OpenStaticPanel { kind } => {
                    // 0x0049743c..0x0049747e: the customization bench.
                    if kind == 0x4d {
                        let ui::GameUi { gui, m, voxel, .. } = &mut self.ui;
                        ui::voxel::open_from_static(gui, m, voxel);
                    }
                }
                Event::OpenCraftingPanel { kind } => {
                    // 0x0049756a..0x004975b6: the crafting stations.
                    let ui::GameUi { gui, m, crafting, inv, .. } = &mut self.ui;
                    ui::crafting::open_from_station(gui, m, crafting, &self.game, &mut inv.crafting, kind);
                }
                Event::InventoryFull => {
                    // 0x00497653..0x004976ae: "You can't carry more of these items." in
                    // (1, 0.2, 0.2) through the ChatWidget (`GC+0x800a14`, 0x0043ab30).
                    self.print("You can't carry more of these items.\n", [255, 51, 51]);
                }
                _ => {}
            }
        }
        self.bubble_events(&talks, &examines);
    }

    /// `0x004889e0` for the aimed creature (R, 0x004973e6): the vendors (class 0x80..0x82)
    /// refresh the shop (0x004a2300) and open it with the bag, on the first tab (`+0x80095c`
    /// `+0x1b4 = 0`), the vendor's inventory copied (`GC+0x800c0c`, `+0x800c18`,
    /// `+0x800d34/38`: read from the vendor's state by [`Controller::refresh_shop`]); the
    /// identifier (0x83) opens the identification panel; the adapter (0x89) the adaption
    /// panel.
    fn open_creature_menu(&mut self, id: i64) {
        let class = {
            let nw = self.net.world.lock().unwrap_or_else(|e| e.into_inner());
            match nw.entities.get(&id) {
                Some(e) => e.0[0x130],
                None => return,
            }
        };
        match class {
            0x83 => {
                let ui::GameUi { gui, m, enchant, .. } = &mut self.ui;
                ui::enchant::on_creature_menu(gui, m, enchant);
            }
            0x80..=0x82 => {
                let m = self.ui.m.clone();
                self.ui.set_visible(m.shop_panel, true);
                self.ui.set_visible(m.inventory_panel, true);
                self.ui.inv.shop.tab = 0;
                self.refresh_shop();
            }
            _ => {}
        }
        if class == 0x89 {
            let ui::GameUi { gui, m, .. } = &mut self.ui;
            ui::adaption::on_creature_menu(gui, m);
        }
    }

    /// The speech bubbles of this frame (`ui::bubbles`), in `update`'s order: the villagers
    /// the player talked to (`0x004882e0(creature, 0)` at 0x00496a4f), the bubbles following
    /// their creatures (0x00496f37..0x00497333), then the examined statics
    /// (`0x00488030(static, 0)` at 0x00497563). The clocks and page steps ran with the HUD
    /// (`present_hud::apply`, 0x0049081b and 0x004966e2).
    fn bubble_events(&mut self, talks: &[i64], examines: &[u32]) {
        for &id in talks {
            self.creature_speech(id, false);
        }
        self.place_bubbles();
        if !examines.is_empty() {
            let pos = {
                let nw = self.net.world.lock().unwrap_or_else(|e| e.into_inner());
                nw.entities.get(&self.player.id).map(scene::entity_pos)
            };
            let Some(pos) = pos else { return };
            for &kind in examines {
                if let Some(i) = ui::bubbles::examine_static(&mut self.ui.bubbles, kind, self.player.id, pos, false) {
                    self.show_bubble(i, true);
                }
            }
        }
    }

    /// Shows or hides bubble `i`'s node (`GC+0x800920[i]`).
    fn show_bubble(&mut self, i: usize, v: bool) {
        let n = self.ui.hud_state.nodes.as_ref().and_then(|h| h.bubbles.get(i).map(|b| b.0));
        if let Some(n) = n {
            self.ui.gui.nodes[n].visible = v;
        }
    }

    /// `0x004882e0(creature, force)` ([`ui::bubbles::creature_speech`]): under the world lock
    /// (its `srand` reseeds the stream the client world's tick draws from), then the map's
    /// visited marks.
    fn creature_speech(&mut self, id: i64, force: bool) {
        let text = self.ui.text.clone();
        let shared = Arc::clone(&self.shared);
        let outcome = {
            let mut world_guard = shared.write_world();
            let world = &mut *world_guard;
            let nw = self.net.world.lock().unwrap_or_else(|e| e.into_inner());
            let Some(e) = nw.entities.get(&id) else { return };
            let render_pos = nw.states.get(&id).map_or_else(|| scene::entity_pos(e), |s| s.riding.render_pos);
            let level = nw.entities.get(&self.player.id).map_or(1, |p| i32::from_le_bytes(p.0[0x180..0x184].try_into().unwrap()));
            let c = ui::bubbles::SpeechCreature { id, entity: e, render_pos };
            let mut rng = std::mem::take(&mut world.rng);
            let o = ui::bubbles::creature_speech(&mut self.ui.bubbles, &c, force, level, text.as_deref(), world, &mut rng);
            world.rng = rng;
            o
        };
        if let Some(i) = outcome.shown {
            self.show_bubble(i, true);
        }
        if !outcome.visited.is_empty() {
            let mut tiles = shared.lock_tiles();
            for (zx, zy) in outcome.visited {
                tiles.mark_visited(zx, zy);
            }
        }
    }

    /// `update` 0x00496f37..0x00497333 ([`ui::bubbles::place_bubble`]) for every bubble with a
    /// creature. A creature that left the world frees its bubble (the original keeps the
    /// dangling `Creature*`).
    fn place_bubbles(&mut self) {
        let Some(nodes) = self.ui.hud_state.nodes.as_ref().map(|h| h.bubbles.clone()) else { return };
        let dt = self.frame_ms;
        let (w, h) = (self.ui.gui.viewport.x, self.ui.gui.viewport.y);
        let net = Arc::clone(&self.net);
        let nw = net.world.lock().unwrap_or_else(|e| e.into_inner());
        let cam = ui::bubbles::BubbleCamera { origin: self.player.camera.origin, view: &self.player.camera.view_shake, projection: &self.player.projection, width: w, height: h };
        for (i, &(n, _)) in nodes.iter().enumerate() {
            let Some(b) = self.ui.bubbles.get_mut(i) else { continue };
            if b.creature == 0 {
                continue;
            }
            let Some(e) = nw.entities.get(&b.creature) else {
                b.creature = 0;
                self.ui.gui.nodes[n].visible = false;
                continue;
            };
            let st = nw.states.get(&b.creature);
            let t = ui::bubbles::BubbleTarget {
                render_pos: st.map_or_else(|| scene::entity_pos(e), |s| s.riding.render_pos),
                step_offset: st.map_or(0.0, |s| s.step_offset),
                height: f32::from_le_bytes(e.0[0x78..0x7c].try_into().unwrap()),
            };
            let pivot = self.ui.gui.nodes[n].pivot;
            match ui::bubbles::place_bubble(b, &t, &cam, dt, pivot) {
                Some(p) => {
                    self.ui.gui.nodes[n].visible = true;
                    self.ui.gui.nodes[n].translation = p;
                }
                None => self.ui.gui.nodes[n].visible = false,
            }
        }
    }

    /// `update` 0x0049cee0..0x0049d110 under lock B: the chunk window origin (the player's
    /// chunk minus half the window, 0x00469060), the player's block position, and the zone
    /// centres (the local player's zone). Also the landscape thread's centre and the map's
    /// visited mark of the player's zone.
    fn publish_thread_inputs(&mut self) {
        let p = {
            let nw = self.net.world.lock().unwrap_or_else(|e| e.into_inner());
            nw.entities.get(&self.player.id).map(scene::entity_pos).unwrap_or([0; 3])
        };
        let (bx, by) = ((p[0] / 65536) as i32, (p[1] / 65536) as i32);
        let half = crate::threads::CHUNK_WINDOW / 2;
        let origin = [bx / 32 - half, by / 32 - half];
        let zone = (bx / 256, by / 256);
        {
            let mut cs = self.shared.lock_world_cs();
            cs.window_origin = origin;
            cs.player_block = [bx, by];
            cs.centres.clear();
            cs.centres.push(zone);
        }
        if let Some(l) = &self.landscape {
            // 0x004695da..0x0046962c: `(int)(GC+0x1000e4c * (1/256) + (float)GC+0x2bc)` per
            // axis: the map pan (blocks, the same field `map_screen_view` adds to the player
            // position) in zones plus the player's zone.
            l.set_centre(landscape_centre([zone.0, zone.1], [self.map_pan[0], self.map_pan[1]]));
        }
        self.shared.lock_tiles().mark_visited(zone.0, zone.1);
        // 0x0048d51c..0x0048d5e5 (`update`, every frame): the landscape tile of the region
        // under the player becomes visible on the far map (`MapTiles::discover_region`).
        let point = self.shared.read_world().nearest_climate_point(bx, by).map(|c| [c.x, c.y]);
        self.shared.lock_tiles().discover_region(point, bx, by, self.world_ms);
    }

    // -----------------------------------------------------------------------------------------
    // render 0x004ac260

    /// The `SceneState` fields that mirror controller fields, refreshed before the gather.
    fn refresh_scene_state(&mut self) {
        let cam = &self.player.camera;
        let m = &self.ui.m;
        let v = |n| self.ui.visible(n);
        self.scene.camera.position = cam.eye;
        self.scene.camera.render_offset = cam.origin;
        self.scene.camera.view = to_d3d(&cam.view_shake.0);
        self.scene.camera.sky_view = to_d3d(&cam.view.0);
        self.scene.camera.pitch = cam.rot[0];
        self.scene.camera.yaw = cam.rot[2];
        self.scene.camera.fog_distance = self.fog_distance;
        self.scene.screen = self.screen;
        self.scene.view_distance = self.view_distance;
        self.scene.clock_ms = self.world_ms;
        self.scene.player = self.player.id;
        self.scene.aimed_item = self.player.aimed_item;
        self.scene.aimed_static = self.player.aimed_static;
        self.scene.frame_time_ms = net::time_get_time() as i32;
        self.scene.engine_time_ms = self.game.engine_time_ms;
        self.scene.gui.w874 = v(m.start_root);
        self.scene.gui.w880 = v(m.char_create_root);
        self.scene.gui.w888 = v(m.char_select_root);
        self.scene.gui.w88c = v(m.world_select_root);
        self.scene.gui.w890 = v(m.world_create_root);
        self.scene.gui.w894 = v(m.server_root);
        self.scene.gui.w898 = v(m.wait);
        self.scene.mode = if self.scene.gui.w898 {
            FrameMode::GuiOnly
        } else if self.game.map_open {
            FrameMode::MapScreen
        } else {
            FrameMode::World
        };
    }

    /// Slot 11 `render()` 0x004ac260, then the device work of `frame` 0x004c85f0 through the
    /// sink.
    pub fn render(&mut self, sink: &mut dyn FrameSink) {
        let gather_scope = profile::scope(Phase::Gather);
        // The nodes cloned this frame (`Node::clone` 0x00636040 copies the template's Display
        // and Transformation) join their template's `.plx` scene.
        let clones = ui::members::take_clone_log();
        if !clones.is_empty() {
            use crate::ui::members::PlxLoader;
            self.plx.register_clones(&self.ui.gui, &clones);
        }
        self.refresh_scene_state();
        // The chunk ring as render sees it (lock C, not nested with the world lock).
        let (window, ring): (passes::ChunkWindow, Vec<RingSnapshot>) = {
            let r = self.shared.lock_mesh_cs();
            (r.window, r.records.iter().map(|c| (c.version, c.coords, c.build.clone())).collect())
        };
        if self.chunk_keys.len() != ring.len() {
            self.chunk_keys = vec![0.0; ring.len()];
        }
        self.scene.window = window;
        self.scene.chunks = ring
            .iter()
            .enumerate()
            .map(|(i, (_, coords, b))| match (coords, b) {
                (Some(c), Some(b)) => SceneChunk {
                    coords: *c,
                    has_buffers: true,
                    aabb_min: b.bounds_min,
                    aabb_max: b.bounds_max,
                    center: chunk_center(b),
                    distance_key: self.chunk_keys[i],
                    props: b.props.clone(),
                },
                _ => SceneChunk { coords: coords.unwrap_or([-1, -1]), distance_key: self.chunk_keys[i], ..SceneChunk::default() },
            })
            .collect();
        let dt = self.frame_ms;
        // `GC+0x800a1c` as the GUI pass sees it: the HUD spin (0x004bbabc) runs after it.
        let gui_rotation = self.scene.item_preview_rotation;
        let shared = Arc::clone(&self.shared);
        let net = Arc::clone(&self.net);
        let mut world_guard = shared.write_world();
        let world = &mut *world_guard;
        let nw = net.world.lock().unwrap_or_else(|e| e.into_inner());
        // `GC+0x800440` is the world.s own clock (`World+0x80015c`; packet 5 writes it when
        // connected, the tick runs it on).
        self.scene.time_of_day_ms = world.time_of_day;
        // The global `rand()` stream is the world's (`cw_world::World::rng` on the client).
        let mut rng: MsvcRand = std::mem::take(&mut world.rng);
        let (pieces, sounds) = scene::gather_scene(world, &nw.entities, &nw.states, &self.projectiles, &mut self.scene, &mut self.particles, &self.models, dt, &mut rng);
        world.rng = rng;
        // 0x004ae27e..0x004ae370 (world frames only): the GUI root `GC+0x800884` is visible
        // exactly when no menu screen is (`scene::GuiVisibility::apply` in the gather).
        if self.scene.mode == FrameMode::World {
            let g = self.ui.m.gui_root;
            self.ui.set_visible(g, pieces.gui.hud);
        }
        for (i, c) in self.scene.chunks.iter().enumerate() {
            if let Some(k) = self.chunk_keys.get_mut(i) {
                *k = c.distance_key;
            }
        }
        drop(gather_scope);
        // The creature bodies: the pose object `creature+0x1498` per creature
        // (0x004b4ef0..0x004b5db2).
        let poses_scope = profile::scope(Phase::Poses);
        let creatures = self.pose_creatures(&nw.entities, &nw.states, &pieces.creature_order, &pieces.body_materials);
        drop(poses_scope);
        let build_scope = profile::scope(Phase::BuildFrame);
        let player_position = nw.entities.get(&self.player.id).map(scene::entity_pos).unwrap_or([0; 3]);
        let map_creatures = map_creatures(&nw.entities, &nw.states);
        drop(nw);
        let cb = cw_render::light::get_block(
            &*world,
            scene::math::block_of_fixed(self.scene.camera.position[0]),
            scene::math::block_of_fixed(self.scene.camera.position[1]),
            scene::math::block_of_fixed(self.scene.camera.position[2]),
        );
        drop(world_guard);
        // The GUI stream and the widgets' item models read the widget tree, not the world:
        // built outside lock A (Tier C; the GUI tessellation is most of a frame's CPU time and
        // held the world from the workers). The world is taken again, for reading, for the
        // frame's world samplers and the map.
        let mut assets = std::mem::take(&mut self.assets);
        let gui = {
            let _g = profile::scope(Phase::Gui);
            crate::assets::gui_frame(self, &mut assets)
        };
        // The widgets' `0x004758c0` item models (`ui::gui_models`).
        let gui_models = ui::gui_models::gui_models(&self.ui, &self.game, &self.ui_out, &ui::gui_models::GuiModelInputs { rotation: gui_rotation, models: &self.models });
        // The select screens' creatures (`CharacterPreviewWidget` 0x00425450,
        // `WorldPreviewWidget` 0x00605ae0, `ui::previews::PreviewModels`).
        let mut gui_creatures = self.preview_models.creatures(&self.ui.gui, &self.ui_out.previews, dt, &self.models);
        let world_meshes = if self.ui.visible(self.ui.m.world_select_root) { self.preview_models.world_meshes(&self.world_previews) } else { Vec::new() };
        gui_creatures.extend(self.preview_models.world_cards(&self.ui.gui, &self.ui_out.previews, &mut self.ui.previews, &self.world_previews, dt, &self.models));
        // 0x004bac82..0x004bb043 (HUD frames only, after the minimap): the Tab wheel, its
        // radius and angle eased by this frame's time.
        let quick_wheel = if pieces.hud_visible {
            let mut wheel = self.quick_wheel;
            let v = wheel.models(&self.ui, &self.game, dt, self.screen, &ui::gui_models::GuiModelInputs { rotation: gui_rotation, models: &self.models });
            self.quick_wheel = wheel;
            v
        } else {
            Vec::new()
        };
        let world_guard = shared.read_world();
        let world: &World = &world_guard;
        let chunks: Vec<ChunkInput> = ring
            .iter()
            .map(|(_, coords, b)| match (coords, b) {
                (Some(c), Some(b)) => ChunkInput {
                    coords: *c,
                    has_buffers: true,
                    aabb_min: b.bounds_min,
                    aabb_max: b.bounds_max,
                    center: chunk_center(b),
                    buffers: b
                        .buffers
                        .iter()
                        .map(|s| ChunkBufferInput {
                            z_base: s.base_z,
                            vertex_count: s.vertices.len() as u32,
                            opaque_primitives: (s.indices.len() / 3) as u32,
                            water_primitives: (s.indices2.len() / 3) as u32,
                            has_opaque: !s.indices.is_empty(),
                            has_water: !s.indices2.is_empty(),
                        })
                        .collect(),
                },
                _ => ChunkInput { coords: coords.unwrap_or([-1, -1]), ..ChunkInput::default() },
            })
            .collect();
        // The prologue 0x004ac2b0..0x004ac312: model-vector slot 0xa08 is the cloud when the
        // vector has it (its size `+0x44`/`+0x48`).
        let (cloud_model, cloud_model_size) = match self.models.models.get(crate::assets::CLOUD_MODEL as usize) {
            Some(m) => (crate::assets::CLOUD_MODEL, [m.size[0], m.size[1]]),
            None => (0, [0, 0]),
        };
        let sampler = WorldView(world);
        let mut tiles = shared.lock_tiles();
        let map_scene = cw_render::map::MapScene {
            tiles: &tiles,
            models: &self.map_models,
            world,
            creatures: &map_creatures,
            player_position,
            state: &self.map_state,
        };
        let inputs = RenderInputs {
            mode: self.scene.mode,
            screen: self.screen,
            white_clear: self.scene.white_clear,
            time_of_day_ms: self.scene.time_of_day_ms,
            clock_ms: self.world_ms,
            frame_ms: dt,
            camera: self.scene.camera.clone(),
            camera_block_kind: cb[3],
            player_position,
            previous_frustum: self.scene.previous_frustum,
            window,
            chunks,
            chunk_props: pieces.chunk_props.clone(),
            lights: pieces.lights.iter().map(|l| l.source()).collect(),
            stars: self.stars.clone(),
            sun_texture: crate::assets::SUN_TEXTURE,
            shadow_texture: crate::assets::SHADOW_TEXTURE,
            cloud_model,
            cloud_model_size,
            objects: pieces.objects.clone(),
            creatures,
            creature_order: pieces.creature_order.clone(),
            creature_extras: pieces.creature_extras.clone(),
            shadows: pieces.shadows.clone(),
            ribbons: pieces.ribbons.clone(),
            far_objects: pieces.far_objects.clone(),
            map_pan: self.map_pan,
            map_zoom: self.player.camera.map_zoom,
            minimap_markers: Vec::new(),
            quick_wheel,
            gui,
            gui_models,
            gui_creatures,
            hud_visible: pieces.hud_visible,
            hud_models: pieces.hud_models.clone(),
            map_compass: pieces.map_compass,
            map: Some(map_scene),
            world: &sampler,
        };
        let frame = passes::build_frame(&inputs);
        // The map's side effects (`cube::WorldMap::draw 0x005fc1b0`: clock, tile fades, stored
        // matrices) for the view this frame drew: the full map (0x004ae004) or the minimap
        // (0x004babaf, when the HUD shows).
        let map_view = match inputs.mode {
            FrameMode::MapScreen => Some(passes::map_screen_view(&inputs)),
            FrameMode::World if inputs.hud_visible => Some(passes::minimap_view(&inputs)),
            _ => None,
        };
        drop(inputs);
        self.assets = assets;
        if let Some(v) = map_view {
            cw_render::map::map_advance(&v, &mut tiles, &mut self.map_state, player_position);
        }
        let mut map_meshes: Vec<(ModelRef, Option<cw_render::mesh::ModelMesh>)> = tiles
            .take_mesh_events()
            .into_iter()
            .map(|ev| match ev {
                cw_render::map::MeshEvent::Added(r) => (r, tiles.mesh(r).cloned()),
                cw_render::map::MeshEvent::Removed(r) => (r, None),
            })
            .collect();
        // The saved worlds' preview models (`WorldInfo+4`) the world cards draw.
        map_meshes.extend(world_meshes);
        drop(tiles);
        drop(world_guard);
        if let Some(p) = frame.frustum_planes {
            self.scene.previous_frustum = p;
        }
        if let Some(p) = frame.projection {
            self.player.projection = player::Mat4(from_d3d(&p));
        }
        // The ambient sounds of the gather, played as `playSound` 0x00484350.
        for s in sounds {
            let l = self.listener();
            self.audio.play_sound(&l, s.id, s.pos, s.volume, s.pitch);
        }
        drop(build_scope);
        let res_chunks: Vec<(u64, Option<Arc<ChunkBuild>>)> = ring.into_iter().map(|(v, _, b)| (v, b)).collect();
        let upload = crate::assets::take_upload(&mut self.assets);
        let resources = FrameResources { chunks: &res_chunks, models: &self.models, map_meshes: &map_meshes, upload };
        sink.render(&frame, resources);
    }

    /// `0x004128f0` for every creature in draw order, with its persistent pose object: the
    /// animation time is the entity's mode time (`+0x5c`), the mode from `anim_mode_for`
    /// (0x004b4ef0), the render position and rotation the tick smooths, the render origin
    /// `GC+0x1d8/+0x1e0`.
    fn pose_creatures(&mut self, entities: &BTreeMap<i64, EntityData>, states: &BTreeMap<i64, CreatureState>, order: &[i64], body_materials: &BTreeMap<i64, [f32; 4]>) -> Vec<CreatureInput> {
        let mut out = Vec::new();
        for &id in order {
            let Some(e) = entities.get(&id) else { continue };
            // 0x004b39ee / 0x004b4e5b: a dead creature (and, on a white-cleared frame, anyone
            // but the player) is not posed or drawn; its pose state is left untouched.
            if !scene::creatures::body_drawn(e, id, self.player.id, self.scene.white_clear) {
                continue;
            }
            let st = states.get(&id);
            let (guard, haste) = cw_sim::util::guard_haste(states, id);
            let walk = self.walk.get(&id).copied().unwrap_or_default();
            let pet = st.map(|s| s.pet).filter(|p| *p != 0).and_then(|p| entities.get(&p));
            // 0x004b5d81..0x004b5db2 (and 0x004b5cf3): the pose colour is the body material
            // `esp+0x7f4` the creature pass computed (ground light 0x004718b0 in the alpha, times
            // the world material). A creature the pass culled (0x004b4e1b) has none and is not
            // drawn by the frame builder either.
            let color = body_materials.get(&id).copied().unwrap_or([1.0; 4]);
            let inputs = PoseInputs {
                models: &self.models,
                dt_ms: self.frame_ms,
                color,
                render_pos: st.map_or(scene::entity_pos(e), |s| s.riding.render_pos),
                render_rot: st.map_or([0.0; 3], |s| s.riding.render_rot),
                origin: self.player.camera.origin,
                step_z: st.map_or(0.0, |s| s.step_offset),
                walk,
                mounted: st.is_some_and(|s| s.modes.mount != 0),
                pet,
                combo: st.map_or(0, |s| s.modes.hit_count_copy),
                charge: st.map_or(0.0, |s| s.charge),
                guard,
                haste,
                hide: 0,
                anim_mode: None,
                hit_flash: st.map_or(0.0, |s| s.ai.hit_flash),
            };
            let time = i32::from_le_bytes(e.0[0x5c..0x60].try_into().unwrap());
            let ps = self.poses.entry(id).or_default();
            let pose = pose::build_pose_with(e, time, &inputs, ps);
            if let Some(w) = self.walk.get_mut(&id) {
                w.blend = pose.walk_blend;
            }
            // The part models and matrices the pose object keeps (`pose+0xbc..`, `+0x16c..`),
            // read by next frame's explosion (0x004b3a31..0x004b4098). Only for a creature the
            // creature pass drew (0x004b4e1b visible), whose `creature+0x1d10` it just set:
            // the original does not call 0x004128f0 on a culled creature.
            if self.scene.explode_pending.contains(&id) {
                let models = &self.models;
                self.scene.pose_parts.entry(id).or_default().record_pose(&pose, |n| models.info(n as i32));
            }
            let parts: Vec<ModelDraw> = pose
                .parts
                .iter()
                .map(|p| {
                    let mut d = ModelDraw::new(0x004128f0, p.model, to_d3d(&p.matrix.to_cols_array()), p.color);
                    d.shininess = p.shine;
                    d.mirrored = p.mirrored;
                    d
                })
                .collect();
            let f = |o: usize| f32::from_le_bytes(e.0[o..o + 4].try_into().unwrap());
            out.push(CreatureInput {
                id,
                position: scene::entity_pos(e),
                height: f(0x78),
                // 0x004ad9d1..0x004ada04: `esp+0xbc = GC+0x1d0 · 0.3`, the limit of the body's
                // sphere test 0x004b4e1b (the scene's `effect_range`).
                cull_distance: self.scene.view_distance * 0.3,
                flash: inputs.hit_flash,
                ghost: st.map_or(0.0, |s| s.block),
                velocity: [f(0x24), f(0x28), f(0x2c)],
                afterimage: AfterimageFields::default(),
                parts,
            });
        }
        self.poses.retain(|k, _| entities.contains_key(k));
        // A deleted creature takes its pose object (and its flag) with it.
        self.scene.pose_parts.retain(|k, _| entities.contains_key(k));
        self.scene.explode_pending.retain(|k| entities.contains_key(k));
        out
    }

    /// `GameController::~GameController` 0x00466d90: the threads stopped, the connection
    /// closed, the map saved.
    pub fn shutdown(&mut self) {
        self.disconnect();
        self.workers.stop();
        if let Some(l) = self.landscape.take() {
            l.stop();
        }
        if let Some(name) = self.map_world.take() {
            let w = self.shared.read_world();
            if let Err(e) = singleplayer::save_map(&self.game_dir, &name, &self.shared.tiles, Some(&w), &mut self.map_blobs) {
                eprintln!("cw-client: map save: {e}");
            }
            // `World::~World`: every loaded zone and region saved (a named world only).
            w.save_all();
        }
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// `creature+0x418`: the first equipment slot the UI shows (`GameView::equipment[0]`, entity
/// `+0x408`).
const EQUIPMENT_AT: usize = 0x408;

/// A raw item of any length as the 0x118 bytes of `cube::Item`.
fn item_bytes(b: &[u8]) -> [u8; Item::SIZE] {
    let mut o = [0u8; Item::SIZE];
    let n = b.len().min(Item::SIZE);
    o[..n].copy_from_slice(&b[..n]);
    o
}

/// A stack as its 0x11c bytes (count, item).
fn stack_bytes(s: &cw_world::inventory::Slot) -> [u8; persist::STACK] {
    let mut b = [0u8; persist::STACK];
    b[0..4].copy_from_slice(&s.count.to_le_bytes());
    b[4..].copy_from_slice(&s.item.to_bytes());
    b
}

/// [`ui::Style`] as the first 0x14 bytes of the `creature+0x1d28` record.
fn style_bytes(st: &ui::Style) -> [u8; 0x14] {
    let mut b = [0u8; 0x14];
    b[0..4].copy_from_slice(&st.race.to_le_bytes());
    b[4] = st.gender;
    b[8..0xc].copy_from_slice(&st.face.to_le_bytes());
    b[0xc..0x10].copy_from_slice(&st.haircut.to_le_bytes());
    b[0x10..0x13].copy_from_slice(&st.hair_color);
    b
}

fn style_from_bytes(b: &[u8; 0x14]) -> ui::Style {
    ui::Style {
        race: i32::from_le_bytes(b[0..4].try_into().unwrap()),
        gender: b[4],
        face: i32::from_le_bytes(b[8..0xc].try_into().unwrap()),
        haircut: i32::from_le_bytes(b[0xc..0x10].try_into().unwrap()),
        hair_color: [b[0x10], b[0x11], b[0x12]],
    }
}

/// `0x0043f7c0(creature+0x64, creature+0x78, style)`: the entity type from the style
/// record's race and gender and the appearance from its variants and hair colour, over the
/// entity's current appearance (fields a case does not write stay).
fn appearance_from_style(e: &mut EntityData, style: &[u8; 0x14], rng: &mut MsvcRand) {
    use cw_world::appearance::{Appearance, AppearanceInfo, init_appearance_with_info};
    let info = AppearanceInfo {
        race: i32::from_le_bytes(style[0..4].try_into().unwrap()),
        gender: style[4],
        variant_a: i32::from_le_bytes(style[8..0xc].try_into().unwrap()),
        variant_b: i32::from_le_bytes(style[0xc..0x10].try_into().unwrap()),
        hair: [style[0x10], style[0x11], style[0x12]],
    };
    let mut entity_type = i32::from_le_bytes(e.0[0x54..0x58].try_into().unwrap());
    let mut app = Appearance::from_bytes(&e.0[0x68..0x68 + Appearance::SIZE]);
    init_appearance_with_info(rng, &mut entity_type, &mut app, &info);
    e.0[0x54..0x58].copy_from_slice(&entity_type.to_le_bytes());
    e.0[0x68..0x68 + Appearance::SIZE].copy_from_slice(&app.to_bytes());
}

/// A saved character's creature for the select screens (`GC+0x800984[i]`): the fields
/// `loadCharacter` 0x004806c0 puts into a creature (as [`Controller::apply_record`] does for
/// the player) at the record's position. The appearance's `rand()` draws (0x0043f7c0) use a
/// throwaway stream so the world's is not advanced by the select screen (an assumption: the
/// original loads these creatures once at startup).
fn preview_entity(r: &CharacterRecord) -> EntityData {
    let mut e = fresh_player_entity();
    for i in 0..3 {
        e.0[i * 8..i * 8 + 8].copy_from_slice(&r.position[i].to_le_bytes());
    }
    e.0[0x18..0x24].copy_from_slice(&r.rotation);
    e.0[0x15c..0x160].copy_from_slice(&r.hp.to_le_bytes());
    e.0[0x184..0x188].copy_from_slice(&r.xp.to_le_bytes());
    e.0[0x180..0x184].copy_from_slice(&r.level.to_le_bytes());
    e.0[0x130] = r.class;
    e.0[0x131] = r.specialization;
    for k in 0..persist::ITEM_SLOTS {
        let o = persist::ITEM_SLOTS_AT + k * persist::ITEM;
        e.0[o..o + persist::ITEM].copy_from_slice(&r.items[k]);
    }
    let n = r.name.len().min(15);
    e.0[0x1158..0x1168].fill(0);
    e.0[0x1158..0x1158 + n].copy_from_slice(&r.name[..n]);
    appearance_from_style(&mut e, &r.style, &mut MsvcRand::new(0));
    e
}

/// The entity type of a record's race and gender (the mapping of the style widget's apply
/// 0x0042c080; human male when the race is out of range).
fn persist_entity_type(r: &CharacterRecord) -> i32 {
    let race = i32::from_le_bytes(r.style[0..4].try_into().unwrap());
    let g = i32::from(r.style[4]);
    match race {
        0 => g + 2,
        1 => g,
        2 => g + 9,
        3 => g + 0xb,
        4 => g + 4,
        5 => g + 7,
        6 => g + 0xf,
        7 => g + 0xd,
        _ => 2,
    }
}

/// The local creature as the constructor sets it up (0x00462145..0x00462196): the entity
/// defaults with the player hostility (`creature+0x60 = 0`), class 1 (`+0x140`), entity type
/// 2 (`+0x64`) and the three item bytes it writes (`+0x991 = 0`, `+0x99d = 1`, `+0x53d = 1`).
pub fn fresh_player_entity() -> EntityData {
    let mut e = EntityData::new_creature();
    // `Appearance::Appearance` (`Server.exe 0x00406970`) at `creature+0x78`.
    e.0[0x68..0x68 + cw_world::appearance::Appearance::SIZE].copy_from_slice(&cw_world::appearance::Appearance::NEW.to_bytes());
    e.0[0x50] = 0;
    e.0[0x130] = 1;
    e.0[0x54..0x58].copy_from_slice(&2i32.to_le_bytes());
    e.0[0x981] = 0;
    e.0[0x98d] = 1;
    e.0[0x52d] = 1;
    e
}

/// The local creature's state outside the entity: full stamina, the four empty bag tabs
/// (`Inventory` 0x00487380(4)).
pub fn fresh_player_state() -> CreatureState {
    let mut st = CreatureState { stamina: 1.0, ..CreatureState::default() };
    st.inventory.pages = vec![Vec::new(); 4];
    st
}

/// The creature map as `cube::WorldMap::draw` reads it (`world+4`, map order), from the
/// creature fields (`creature+X` is `entity+(X-0x10)`).
fn map_creatures(entities: &BTreeMap<i64, EntityData>, states: &BTreeMap<i64, CreatureState>) -> Vec<cw_render::map::MapCreature> {
    entities
        .iter()
        .map(|(id, e)| {
            let st = states.get(id);
            let u16_at = |o: usize| u16::from_le_bytes([e.0[o], e.0[o + 1]]);
            cw_render::map::MapCreature {
                position: scene::entity_pos(e),
                render_position: st.map_or(scene::entity_pos(e), |s| s.riding.render_pos),
                yaw: st.map_or(0.0, |s| s.riding.render_rot[2]),
                hostility: e.0[0x50],
                flags: u32::from(u16_at(0x6e)),
                race: e.0[0x130],
                head_model: u16_at(0x7c),
                hair_model: u16_at(0x7e),
                hair_color: [e.0[0x6a], e.0[0x6b], e.0[0x6c]],
            }
        })
        .collect()
}

/// One ring record as `render` copies it out of lock C: version, coordinates, meshes.
type RingSnapshot = (u64, Option<[i32; 2]>, Option<Arc<ChunkBuild>>);

/// `chunk+0x50`: the centre the distance key uses (the middle of the chunk's box).
fn chunk_center(b: &ChunkBuild) -> [i64; 3] {
    [(b.bounds_min[0] + b.bounds_max[0]) / 2, (b.bounds_min[1] + b.bounds_max[1]) / 2, (b.bounds_min[2] + b.bounds_max[2]) / 2]
}

/// A `float[16]` of the original (row-major, row vectors) as a [`D3dMatrix`].
pub fn to_d3d(m: &[f32; 16]) -> D3dMatrix {
    [[m[0], m[1], m[2], m[3]], [m[4], m[5], m[6], m[7]], [m[8], m[9], m[10], m[11]], [m[12], m[13], m[14], m[15]]]
}

/// The inverse of [`to_d3d`].
pub fn from_d3d(m: &D3dMatrix) -> [f32; 16] {
    let mut o = [0f32; 16];
    for r in 0..4 {
        for c in 0..4 {
            o[r * 4 + c] = m[r][c];
        }
    }
    o
}

/// A shared handle for tests and tools that need the controller's model table without a
/// window.
pub fn empty_models() -> ModelCache {
    ModelCache { models: Arc::new(Vec::new()) }
}

/// Keeps `Mutex` in the imports for the lock docs.
#[allow(unused)]
type _Doc = Mutex<()>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrices_round_trip() {
        let a: [f32; 16] = core::array::from_fn(|i| i as f32);
        assert_eq!(from_d3d(&to_d3d(&a)), a);
        assert_eq!(to_d3d(&a)[3][0], 12.0);
    }

    /// Slot 8 `onMouseUp` 0x0047ddd0 reaches the system menu (0x0047e00e): "Exit Game" sets
    /// the quit flag `GC+0x1a0`, "Options" shows the options panel. Skipped without
    /// `CW_GAME_DIR`.
    #[test]
    fn system_menu_buttons_act_on_release() {
        let Some(dir) = std::env::var_os("CW_GAME_DIR").map(PathBuf::from) else {
            eprintln!("CW_GAME_DIR not set; skipped");
            return;
        };
        let mut c = Controller::new(dir, [1280, 720], vec![(1280, 720)], false, String::new());
        let m = c.ui.m.clone();
        assert!(m.system_panel.is_some());
        // "Options".
        c.ui.set_visible(m.system_panel, true);
        c.ui.system_menu_state.hovered = 0;
        c.on_mouse_down(MouseButton::Left);
        c.on_mouse_up(MouseButton::Left);
        assert!(!c.ui.visible(m.system_panel));
        assert!(c.ui.visible(m.options_panel));
        c.ui.set_visible(m.options_panel, false);
        // "Exit Game".
        c.ui.set_visible(m.system_panel, true);
        c.ui.system_menu_state.hovered = 2;
        c.on_mouse_down(MouseButton::Left);
        assert!(!c.quit);
        c.on_mouse_up(MouseButton::Left);
        assert!(c.quit);
    }

    /// R on a vendor villager (0x00496a4f and 0x004973e6): its speech bubble is written
    /// (`0x004882e0`) and the shop opens with its stock (`0x004889e0`). Skipped without
    /// `CW_GAME_DIR`.
    #[test]
    fn talking_to_a_vendor_shows_a_bubble_and_the_shop() {
        let Some(dir) = std::env::var_os("CW_GAME_DIR").map(PathBuf::from) else {
            eprintln!("CW_GAME_DIR not set; skipped");
            return;
        };
        let mut c = Controller::new(dir, [1280, 720], vec![(1280, 720)], false, String::new());
        let m = c.ui.m.clone();
        for n in [m.start_root, m.char_select_root, m.char_create_root, m.world_select_root, m.world_create_root, m.server_root] {
            c.ui.set_visible(n, false);
        }
        c.ui.set_visible(m.gui_root, true);
        // One UI frame builds the HUD nodes (the four bubbles).
        c.ui_frame(16);
        assert_eq!(c.ui.bubbles.len(), 4);
        let vid = 7i64;
        {
            let mut nw = c.net.world.lock().unwrap();
            let mut v = nw.entities[&LOCAL_PLAYER_ID].clone();
            v.0[0x50] = 3;
            v.0[0x130] = 0x80;
            v.0[0x1c8] = 0;
            nw.entities.insert(vid, v);
            let mut st = CreatureState::default();
            let mut potion = Item::NEW;
            potion.item_type = 1;
            potion.sub_type = 1;
            st.inventory.pages = vec![vec![cw_world::inventory::Slot { count: 3, item: potion }]];
            nw.states.insert(vid, st);
        }
        c.player.menu_creature = vid;
        c.player.events.push(Event::Talk { id: vid });
        c.player.events.push(Event::CreatureMenu { id: vid });
        c.player_events();
        let b = c.ui.bubbles.iter().find(|b| b.creature == vid).expect("a bubble follows the vendor");
        assert!(b.texts.first().is_some_and(|l| !l.is_empty()), "{:?}", b.texts);
        assert!(c.ui.visible(m.shop_panel) && c.ui.visible(m.inventory_panel));
        assert_eq!(c.ui.inv.shop_data.pages.first().map(|p| p.len()), Some(1));
        // "Examine" on a campfire: "There is nothing special." over the player.
        c.player.events.push(Event::Examine { kind: 0x41 });
        c.player_events();
        assert!(c.ui.bubbles.iter().any(|b| b.creature == LOCAL_PLAYER_ID));
    }

    /// Chat typed offline (0x0047e652..0x0047e6b0) goes onto the received list under the
    /// player's id and comes out one entry per frame (0x0048cc0f..0x0048cd99) as
    /// `"<name>: "` then the text and `"\n"`: one line per message. Skipped without
    /// `CW_GAME_DIR`.
    #[test]
    fn own_chat_is_named_one_line_each() {
        let Some(dir) = std::env::var_os("CW_GAME_DIR").map(PathBuf::from) else {
            eprintln!("CW_GAME_DIR not set; skipped");
            return;
        };
        let mut c = Controller::new(dir, [1280, 720], vec![(1280, 720)], false, String::new());
        let m = c.ui.m.clone();
        for n in [m.start_root, m.char_select_root, m.char_create_root, m.world_select_root, m.world_create_root, m.server_root] {
            c.ui.set_visible(n, false);
        }
        c.ui.set_visible(m.gui_root, true);
        let set_name = |e: &mut EntityData| {
            e.0[0x1158..0x1168].fill(0);
            e.0[0x1158..0x115e].copy_from_slice(b"Player");
        };
        set_name(&mut c.game.player);
        set_name(c.net.world.lock().unwrap().entities.get_mut(&c.player.id).unwrap());
        c.ui.chat.lines.clear();
        for msg in ["hello", "again"] {
            c.ui.chat.input_active = true;
            for ch in msg.encode_utf16() {
                c.on_char(ch);
            }
            for a in c.ui.chat.enter(&c.game) {
                c.apply_action(a);
            }
        }
        c.ui_frame(16);
        c.ui_frame(16);
        let lines: Vec<String> = c.ui.chat.lines.iter().map(|l| l.iter().map(|t| t.text.as_str()).collect()).collect();
        assert_eq!(lines, ["Player: hello", "Player: again", ""]);
    }

    #[test]
    fn model_cache_bounds() {
        let c = empty_models();
        assert!(c.model(0).is_none());
        assert!(c.model(-1).is_none());
        assert_eq!(pose::ModelSource::model_count(&c), 0);
    }
}
