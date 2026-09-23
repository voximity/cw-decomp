//! What `GameController::render` (`Cube.exe 0x004ac260`) gathers for its passes: the parts of
//! the 59 KB function that walk the world and the controller's lists and turn them into the
//! inputs of `cw_render::passes::build_frame` (lights, prop and object models, particles,
//! shadows, ribbons, HUD models), plus the side effects that consume the global `rand()`
//! (ambient bird/cricket/owl sounds, falling leaves, model explosions, beam sparks), which the
//! port runs here, in the original's order, before the frame is built (see `docs/TODO.md`,
//! "rand() inside the original's render").
//!
//! Tier B: operation order, comparison directions, truncations and every `rand()` draw follow
//! the assembly; the pieces are pure data for the Tier C renderer.
//!
//! # Map of the original
//!
//! | Range | Piece | Here |
//! |---|---|---|
//! | `0x004ac4c7..0x004ad434` | chunk window walk: distance key `+0x6c` of visible chunks (previous frame's planes), light-emitting props, ambient sounds (`rand()`) | [`chunk_walk`] |
//! | `0x004ae27e..0x004ae370` | HUD widget visibility | [`GuiVisibility::apply`] |
//! | `0x004aee2e..0x004af353` | this frame's frustum planes (stored to `GC+0x1000fa4` before the gather below) | `cw_render::passes::frustum_planes` |
//! | `0x004af353..0x004b0e84` | dynamic lights: projectiles, flash lights, creatures, zone statics and ground items; then `std::sort` of the whole list by distance | [`gather_lights`] |
//! | `0x004b1a6b..0x004b21ac` | terrain pass per visible chunk: falling leaves (`rand()`), near props `0x004bd160`, far props to the far list `0x004c1190` | [`terrain_props`] |
//! | `0x004b21ac..0x004b32ed` | zones around the player: ground items, statics (beams `0x00471d50`, `0x004be760`, camp fires `0x004bbd80`), far statics to the far list | [`items`], [`statics`] |
//! | `0x004b32ed..0x004b35d4` | far list sorted far → near (`0x004abac0`); target marker | [`gather_scene`] |
//! | `0x004b35d4..0x004b8bc7` | creature pass: airships, creature list and order, explosions (`0x00470d80`), beams, projectiles, particles (`0x004b7db6..0x004b8138`) | [`creatures`], [`crate::particles::ParticleSystem::draw`] |
//! | `0x004b8bc7..0x004b9993` | blob shadows | [`fx`] |
//! | `0x004b9993..0x004ba13e` | ribbons | [`fx`] |
//! | `0x004ba462..0x004ba5d0` | the far list, alpha-faded, after the water | [`far_draws`] |
//! | `0x004baa67..0x004baa6d` | `0x00632870(GC+0x800754)`: destroy the children of the per-frame overlay node | [`ScenePieces::clear_overlay_nodes`] |
//! | `0x004bb080..0x004bbb1a` | HUD models | [`hud`] |

// The loops follow the original's indexing.
#![allow(clippy::needless_range_loop)]
// `!(a > b)` keeps the original's NaN behaviour (`comiss` + `jbe`).
#![allow(clippy::neg_cmp_op_on_partial_ord)]
#![allow(dead_code)]

pub mod beam;
pub mod creatures;
pub mod fx;
pub mod hud;
pub mod items;
pub mod math;
pub mod statics;

use std::collections::BTreeMap;

use cw_math::rand::MsvcRand;
use cw_net::EntityData;
use cw_render::frame::{D3dMatrix, FixedVertex, ModelRef, IDENTITY};
use cw_render::mesh::VoxelGrid;
use cw_render::passes::{
    CameraInputs, ChunkWindow, EmitterInput, HudModel, ModelDraw, PointLightSource, box_visible, draw_distance,
    sphere_visible,
};
use cw_sim::combat::CreatureState;
use cw_world::World;
use cw_world::inventory::Item;
use cw_world::zone::Prop;

use crate::particles::{ParticleSystem, leaf_particle};
use math::*;

// ---------------------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------------------

/// A voxel model as the scene needs it: the renderer's handle and the voxel size
/// (`Model+0x44`, `+0x48`, `+0x4c`: x, y, z).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelInfo {
    /// Handle the renderer knows.
    pub handle: ModelRef,
    /// Size in voxels.
    pub size: [i32; 3],
}

/// The controller's model tables. The original holds `cube::Model*` pointers; the port asks
/// the owner of the model meshes.
pub trait SceneModels {
    /// `0x004120c0(GC+0x300, index)`: the model vector (bounds-checked, `None` for a null
    /// slot).
    fn model(&self, index: i32) -> Option<ModelInfo>;
    /// `GC+0x800718[kind]`: the prop model table (`None` for a null slot or out of range).
    fn prop_model(&self, kind: u32) -> Option<ModelInfo>;
    /// `std::vector::size` of `GC+0x800718` (`0x00487f50`).
    fn prop_model_count(&self) -> usize;
    /// `0x004ec400`: the model of an item (`None` for an item without one). See [`items`].
    fn item_model(&self, item: &Item) -> Option<ModelInfo>;
    /// The voxels of a model (`Model+0x30` and its size), for `0x00470d80`.
    fn voxels(&self, model: ModelRef) -> Option<VoxelGrid<'_>>;
    /// `GC+0x800730`: the 1x1x1 cube the particles are drawn with.
    fn particle_cube(&self) -> ModelRef;
    /// `0x00598840(World, key)`: the build-mode models (`std::map<int, Model*>` at
    /// `World+0x800154`), keyed by the local player's build selection (`entity+0x17d`).
    fn build_model(&self, _key: i32) -> Option<ModelInfo> {
        None
    }
}

/// One record of the chunk ring (`GC+0x2e0`, 0x268 bytes) as the gather reads it.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct SceneChunk {
    /// `+0x18`: chunk coordinates the record holds.
    pub coords: [i32; 2],
    /// `+0xc != 0`: the record has buffers.
    pub has_buffers: bool,
    /// `+0x20`, `+0x38`: bounding box, world fixed point.
    pub aabb_min: [i64; 3],
    /// See `aabb_min`.
    pub aabb_max: [i64; 3],
    /// `+0x50`: centre for the distance key.
    pub center: [i64; 3],
    /// `+0x6c`: squared distance from the camera in blocks², rewritten only while the chunk
    /// is visible (so it keeps a stale value otherwise, which the ambient sounds read).
    pub distance_key: f32,
    /// `+0x240`: the chunk's props (`cw_render::mesh::chunk_props`: copies of the zone's
    /// props with `f28` set to their light), in list order.
    pub props: Vec<Prop>,
}

/// What the gather needs of the pose object at `creature+0x1498` (the pose module's
/// `cw_render::pose::PoseState`, copied after `build_pose_with`).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct PoseView {
    /// `pose+0xac` (`creature+0x1544`, `PoseState::root_off`): the blob shadow's centre offset.
    pub root_off: [f32; 3],
    /// `pose+0x56c`, `+0x62c`, `+0x6ec`, `+0x7ac` (`creature+0x1a04`, `+0x1ac4`, `+0x1b84`,
    /// `+0x1c44`, `PoseState::trails`): right tip, right base, left tip, left base, 16 points
    /// each, newest first, as stored (relative to `render_pos + origin` in x and y,
    /// `0x004225fd`).
    pub trails: [[[f32; 3]; 16]; 4],
    /// `creature+0x1d18`, `+0x1d20` (`pose+0x880`, `+0x888`, `PoseState::origin`): the render
    /// offset of that frame.
    pub origin: [i64; 2],
}

impl PoseView {
    /// `creature+0x1a04`: the right tip's newest point.
    pub fn right_tip(&self) -> [f32; 3] {
        self.trails[0][0]
    }
    /// `creature+0x1b84`: the left tip's newest point.
    pub fn left_tip(&self) -> [f32; 3] {
        self.trails[2][0]
    }
}

/// A short-lived light of `GC+0x800748` (0x30 bytes): hit flashes and explosions pushed by
/// `GameController::update` (`0x004863d0`, e.g. `0x00494ded`: colour (0.3, 0.3, 0.5), radius
/// 16, 400 ms), aged there (`0x004963cb..0x0049645b`: `age += dt`, removed when
/// `age > duration`).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct FlashLight {
    /// `+0x00`: position, world fixed point.
    pub position: [i64; 3],
    /// `+0x18`: colour.
    pub color: [f32; 3],
    /// `+0x24`: radius.
    pub radius: f32,
    /// `+0x28`: age in ms.
    pub age: i32,
    /// `+0x2c`: duration in ms.
    pub duration: i32,
}

impl FlashLight {
    /// `GameController::update 0x0049640b`: `age += dt`; `true` when it is past its duration
    /// and must be removed.
    pub fn advance(&mut self, dt_ms: i32) -> bool {
        self.age = self.age.wrapping_add(dt_ms);
        self.age > self.duration
    }
}

/// The widgets whose visibility `render` reads or writes (`Widget+0x3c → +0x94[+0x68]`,
/// `0x0047fa10`). Their roles are not identified; they are named by controller offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct GuiVisibility {
    /// `GC+0x800874`.
    pub w874: bool,
    /// `GC+0x800880`.
    pub w880: bool,
    /// `GC+0x800884`: the HUD (minimap, HUD models), written by [`GuiVisibility::apply`].
    pub hud: bool,
    /// `GC+0x800888`.
    pub w888: bool,
    /// `GC+0x80088c`.
    pub w88c: bool,
    /// `GC+0x800890`.
    pub w890: bool,
    /// `GC+0x800894`.
    pub w894: bool,
    /// `GC+0x800898`: the loading/start screen.
    pub w898: bool,
    /// `GC+0x8008a0`: hidden every frame by [`GuiVisibility::apply`].
    pub w8a0: bool,
}

impl GuiVisibility {
    /// `0x004ae27e..0x004ae370`: the HUD is visible exactly when none of the panels 874, 880,
    /// 888, 894, 88c, 890, 898 is; widget 8a0 is hidden.
    pub fn apply(&mut self) {
        self.hud = !(self.w874 || self.w880 || self.w888 || self.w894 || self.w88c || self.w890 || self.w898);
        self.w8a0 = false;
    }

    /// `0x004b38fd..0x004b3940`: with panel 874, 888, 894 or 88c open no creature is drawn
    /// (the creature list is cleared, `0x0044be20`).
    pub fn hides_creatures(&self) -> bool {
        self.w874 || self.w888 || self.w894 || self.w88c
    }
}

/// The `GameController` fields the gather reads (and the few it keeps between frames).
#[derive(Clone, Debug, PartialEq)]
pub struct SceneState {
    /// Camera: `GC+0x140` position, `+0x1d8`/`+0x1e0` render offset, `+0x26c` view, `+0x1d4`
    /// fog distance, ...
    pub camera: CameraInputs,
    /// `GC+0x11c`, `+0x120`: client size in pixels (this frame's projection and planes).
    pub screen: [i32; 2],
    /// `GC+0x1d0`: the prop view distance (`render` keeps `0.2×` and `0.3×` of it at
    /// `esp+0xac`/`esp+0xbc`: props fade out between `0.18×` and `0.2×`).
    pub view_distance: f32,
    /// `GC+0x8003a0` (`World+0x8000bc`): the millisecond clock (light flicker, sway, sounds).
    pub clock_ms: i32,
    /// `GC+0x800440` (`World+0x80015c`): time of day in ms.
    pub time_of_day_ms: i32,
    /// `GC+0x8006e6`: white clear (no terrain, no ground items or statics).
    pub white_clear: bool,
    /// `GC+0x2ac`, `+0x2b0`, `+0x2dc`: the chunk ring window.
    pub window: ChunkWindow,
    /// `GC+0x2e0`: the ring, index `(y % n) * n + x % n`.
    pub chunks: Vec<SceneChunk>,
    /// `GC+0x1000fa4`: last frame's frustum planes (the chunk walk runs before this frame's
    /// are computed).
    pub previous_frustum: [[f32; 4]; 6],
    /// `GC+0x8006d0`: the local player's creature id.
    pub player: i64,
    /// `GC+0x800748`: flash lights.
    pub flash_lights: Vec<FlashLight>,
    /// Widget visibility.
    pub gui: GuiVisibility,
    /// Per creature: its pose object's trails and origin.
    pub poses: BTreeMap<i64, PoseView>,
    /// The global `0x0076b17c`: a 100 ms timer `render` advances by the frame time and resets
    /// once it passes 100 (`0x004b06fc..0x004b0756`); its flag is not used by the code traced.
    pub timer_76b17c: i32,
    /// `GC+0x800704`: draw the target marker (`0x004b3338`).
    pub target_marker: bool,
    /// Frame shape: the loading screen (widget `GC+0x800898`, `0x004ad43a`) and the map
    /// screen (`GC+0x8006e4`, `0x004adede`) leave `render` right after the chunk walk (the
    /// ambient sounds still play) and the sky.
    pub mode: cw_render::passes::FrameMode,
    /// `0x0047fa10(0x00487490(GC+0x800710))`: the GUI engine's root widget (`Engine+0xb4`) is
    /// visible (with `GC+0x800884` it gates the minimap and the HUD models).
    pub engine_root_visible: bool,
    /// `Engine+0xe8` (`0x0043a490(GC+0x800710)`): the GUI engine's clock in ms (the portrait
    /// wobble).
    pub engine_time_ms: i32,
    /// `std::vector::size(GC+0x800778)`: the other-player frames of the HUD (3 from the
    /// constructor, `0x0046128c`).
    pub friend_frames: i32,
    /// `GC+0x800a1c`: the item-preview rotation (`0x004758c0` at `0x004bb01d`), advanced by
    /// every HUD frame (`hud::item_preview_spin`, `0x004bbabc..0x004bbb16`).
    pub item_preview_rotation: [f32; 3],
    /// `GC+0x800a78`: the ground item under the crosshair (zone x, zone y, index; -1s for
    /// none; `LocalPlayer::aimed_item`), highlighted by `items::zone_items`.
    pub aimed_item: [i32; 3],
    /// `GC+0x800a84`: the static under the crosshair (zone x, zone y, index; `(-1, -1, 0)`
    /// from the constructor; `LocalPlayer::aimed_static`), highlighted by `statics`.
    pub aimed_static: [i32; 3],
    /// `timeGetTime()` read at `render` entry (`0x004ac2aa`, `esp+0x34`): the camp fires'
    /// clock (`0x004bbd80`).
    pub frame_time_ms: i32,
    /// Per creature, the pose's part models and matrices of its last frame
    /// (`creature+0x1554..`, `+0x1604..`): explosions, spirit cubes, ray origins.
    pub pose_parts: BTreeMap<i64, creatures::PoseParts>,
    /// `creature+0x1d10 != 0` (the byte `pose+0x878`; 0 from the pose constructor at
    /// 0x00412049): the pose was drawn since the creature pass last cleared the flag, i.e. on
    /// the previous frame. Set by `0x004128f0` at 0x0041293b ([`creatures::PassEffects::posed`]),
    /// read by the dead branch at 0x004b3a31 (the creature bursts into cubes on its first
    /// frame with `hp <= 0`), cleared for every listed creature at 0x004b411d / 0x004b4129.
    pub explode_pending: std::collections::BTreeSet<i64>,
    /// `0x00602440(GC+0x800d44, zx, zy)` record `+0x30 & 1` per zone that has a record (the
    /// world map's "discovered" flag); zones without a record are absent.
    pub zone_discovered: BTreeMap<(i32, i32), bool>,
    /// `GC+0x1001004`: draw the path finder's nodes (debug).
    pub show_paths: bool,
    /// `World+0xc` (`GC+0x2f0`): airships (none in the port's simulation).
    pub airships: Vec<creatures::Airship>,
}

impl Default for SceneState {
    fn default() -> Self {
        SceneState {
            camera: CameraInputs {
                position: [0; 3],
                render_offset: [0; 2],
                view: IDENTITY,
                sky_view: IDENTITY,
                pitch: 0.0,
                yaw: 0.0,
                narrow_fov: false,
                fog_distance: 100.0,
            },
            screen: [1280, 720],
            view_distance: 100.0,
            clock_ms: 0,
            time_of_day_ms: 43_200_000,
            white_clear: false,
            window: ChunkWindow::default(),
            chunks: Vec::new(),
            previous_frustum: [[0.0; 4]; 6],
            player: 0,
            flash_lights: Vec::new(),
            gui: GuiVisibility::default(),
            poses: BTreeMap::new(),
            timer_76b17c: 0,
            target_marker: false,
            mode: cw_render::passes::FrameMode::World,
            engine_root_visible: true,
            engine_time_ms: 0,
            friend_frames: 3,
            item_preview_rotation: [0.0; 3],
            aimed_item: [-1; 3],
            aimed_static: [-1, -1, 0],
            frame_time_ms: 0,
            pose_parts: BTreeMap::new(),
            explode_pending: Default::default(),
            zone_discovered: BTreeMap::new(),
            show_paths: false,
            airships: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------------------
// Outputs
// ---------------------------------------------------------------------------------------

/// A sound `render` plays through `GameController::playSound 0x00484350` (`id`, position,
/// volume, pitch as the frequency ratio). The controller hands it to `audio::compute_cue`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SoundCue {
    /// Sound id (0x5e..0x64: bird1–3, cricket1–2, owl1–2).
    pub id: u32,
    /// World fixed-point position.
    pub pos: [i64; 3],
    /// Volume argument.
    pub volume: f32,
    /// Pitch argument.
    pub pitch: f32,
}

/// One point light of the gather (the 0x30-byte record of the light vector: position,
/// radius `+0x18`, colour `+0x1c`, distance key `+0x28`).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct SceneLight {
    /// World fixed-point position.
    pub position: [i64; 3],
    /// Radius in blocks.
    pub radius: f32,
    /// Colour.
    pub color: [f32; 3],
    /// `+0x28`: squared distance from the camera in blocks² (the sort key).
    pub distance: f32,
}

impl SceneLight {
    /// The renderer's view of the light.
    pub fn source(&self) -> PointLightSource {
        PointLightSource { position: self.position, radius: self.radius, color: self.color }
    }
}

/// An entry of the far list (`0x004c1190`, 16 bytes): a prop or a static too far to draw
/// in its pass, drawn faded after the water (`0x004ba462..0x004ba5d0`).
#[derive(Clone, Debug, PartialEq)]
pub enum FarObject {
    /// A chunk prop.
    Prop {
        /// The prop.
        prop: Prop,
        /// Its index in its chunk's list, from 1 (the sway phase).
        index: i32,
    },
    /// A zone static.
    Static {
        /// Zone coordinates.
        zone: (i32, i32),
        /// Index in the zone's statics.
        index: usize,
    },
}

/// A far-list record with its key (`+8`: squared distance in blocks²).
#[derive(Clone, Debug, PartialEq)]
pub struct FarEntry {
    /// The object.
    pub object: FarObject,
    /// `+8`: squared distance from the camera.
    pub dist2: f32,
}

/// Everything the gather produces for one frame: the pieces of
/// `cw_render::passes::RenderInputs` it owns, and the fields the report asks `RenderInputs`
/// to grow.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ScenePieces {
    /// All point lights (static emitters and dynamic lights) sorted near → far, as the
    /// original assigns them (`0x004abae0` before `0x004b0e84`). `build_frame` currently
    /// takes only the dynamic ones, unsorted, after its own emitter lights: see the report.
    pub lights: Vec<SceneLight>,
    /// The dynamic lights alone in gather order (`RenderInputs::dynamic_lights` as it is).
    pub dynamic_lights: Vec<PointLightSource>,
    /// Per ring record: the light emitters of its props (`RenderInputs::chunks[i].emitters`).
    pub chunk_emitters: Vec<Vec<EmitterInput>>,
    /// Per ring record: near props drawn right after the chunk (`RenderInputs::chunks[i].props`).
    pub chunk_props: Vec<Vec<ModelDraw>>,
    /// Ground items, statics, the target marker (`RenderInputs::objects`).
    pub objects: Vec<ModelDraw>,
    /// Creature ids in draw order (far → near), empty while a big panel is open.
    pub creature_order: Vec<i64>,
    /// Per creature whose body the pass draws, the body material (`esp+0x7f4`, the ground
    /// light × the world material, 0x004b4e70..0x004b5b44): the colour argument of the pose
    /// draw `0x004128f0` (0x004b5cf3, 0x004b5db2). See [`creatures::body_material`].
    pub body_materials: BTreeMap<i64, [f32; 4]>,
    /// Airships, projectiles, beams and other models of the creature pass
    /// (`RenderInputs::creature_extras`), including the particles.
    pub creature_extras: Vec<ModelDraw>,
    /// Blob shadows (`RenderInputs::shadows`).
    pub shadows: Vec<[FixedVertex; 4]>,
    /// Ribbons (`RenderInputs::ribbons`).
    pub ribbons: Vec<Vec<FixedVertex>>,
    /// Far props and statics after the water, alpha-faded (new `RenderInputs` field).
    pub far_objects: Vec<ModelDraw>,
    /// `GC+0x800884` visible after [`GuiVisibility::apply`] (`RenderInputs::hud_visible`).
    pub hud_visible: bool,
    /// HUD models (`RenderInputs::hud_models`), drawn with the HUD light
    /// ([`hud::HUD_LIGHT`], `0x004bb259`) and no point lights.
    pub hud_models: Vec<HudModel>,
    /// The map screen's compass (`0x004ae21b`), drawn after the map and a depth clear with
    /// [`hud::MAP_COMPASS_LIGHT`] (new `RenderInputs` field).
    pub map_compass: Option<HudModel>,
    /// The widget visibility after this frame's writes (for the GUI layer).
    pub gui: GuiVisibility,
    /// `0x00632870(GC+0x800754)` ran: the GUI layer must destroy the per-frame overlay nodes
    /// before the GUI pass (always true for a world frame).
    pub clear_overlay_nodes: bool,
}

/// Read-only context shared by the gather's parts.
pub struct SceneCtx<'a> {
    /// The client's world.
    pub world: &'a World,
    /// Entity blocks by id (the creature map, `World+4`).
    pub entities: &'a BTreeMap<i64, EntityData>,
    /// Creature state outside the entity block.
    pub states: &'a BTreeMap<i64, CreatureState>,
    /// Controller fields.
    pub gc: &'a SceneState,
    /// Model tables.
    pub models: &'a dyn SceneModels,
    /// The projectiles (`World+0x14`, `GC+0x2f8`).
    pub projectiles: &'a [cw_sim::projectile::Projectile],
    /// This frame's frustum planes.
    pub planes: [[f32; 4]; 6],
    /// Daylight factor (`esp+0xc4`, `0x004ac394..0x004ac46e`).
    pub daylight: f32,
    /// The camera is in water.
    pub underwater: bool,
    /// `esp+0x73c`: `(1, 1, 1, d)` or the underwater `(0.4, 0.5, 1, d)`.
    pub world_material: [f32; 4],
    /// `esp+0xc0`: `GC+0x1d4 + 22.4`.
    pub draw_distance: f32,
    /// `esp+0xac`: `GC+0x1d0 × 0.2`, the props' far distance.
    pub prop_far: f32,
    /// `esp+0x60`: `prop_far × 0.9`, the props' near distance.
    pub prop_near: f32,
    /// `esp+0x4c`: `draw_distance × 0.3`, the statics' far distance.
    pub static_far: f32,
    /// `esp+0x3c`: `draw_distance × 0.3 × 0.9`, the statics' near distance.
    pub static_near: f32,
    /// `esp+0x64`: `draw_distance × 0.2`, the particles' range.
    pub particle_range: f32,
    /// Frame time in ms (`GC+0x8006e8`).
    pub dt_ms: i32,
}

// ---------------------------------------------------------------------------------------
// The gather
// ---------------------------------------------------------------------------------------

fn ring_index(w: &ChunkWindow, x: i32, y: i32) -> usize {
    let n = w.size;
    ((y % n) * n + x % n) as usize
}

/// `(1 - (2t/86.4e6 - 1)^4)^5`, `0x004ac394..0x004ac46e` (the same as `passes::daylight`).
pub fn daylight(time_of_day_ms: i32) -> f32 {
    cw_render::passes::daylight(time_of_day_ms)
}

/// `0x004ac4c7..0x004ad434`: the chunk window walk (x outer, y inner, within `0..0x80000`).
///
/// For a record that has buffers and holds the chunk the window expects, the box test
/// (`0x0047f3c0`, previous frame's planes, distance `draw_distance + 32`) refreshes its key
/// `+0x6c`. Then, for every prop of the record:
///
/// - light (`flags & 1` and the sphere test with margin 16): radius
///   `cos(clock·0.01 + 33n)·0.5 + 15` (`n` counts the record's lit props from 1), colour
///   `prop+0x2c` (× `1 - d/2` for kind 0xd), key = squared distance from the camera;
/// - sound, while the record's key is below 4096 (64 blocks): see [`ambient_sound`].
///
/// Returns the emitter lights in walk order, the emitters per record, and pushes the cues.
pub fn chunk_walk(
    gc: &mut SceneState,
    daylight: f32,
    dt_ms: i32,
    rng: &mut MsvcRand,
    sounds: &mut Vec<SoundCue>,
) -> (Vec<SceneLight>, Vec<Vec<EmitterInput>>) {
    let mut lights = Vec::new();
    let mut emitters = vec![Vec::new(); gc.chunks.len()];
    let w = gc.window;
    if w.size <= 0 {
        return (lights, emitters);
    }
    let dist = draw_distance(&gc.camera);
    for x in w.origin[0]..w.origin[0] + w.size {
        for y in w.origin[1]..w.origin[1] + w.size {
            if x < 0 || y < 0 || x >= 0x80000 || y >= 0x80000 {
                continue;
            }
            let idx = ring_index(&w, x, y);
            let camera = gc.camera.clone();
            let planes = gc.previous_frustum;
            let clock = gc.clock_ms;
            let tod = gc.time_of_day_ms;
            let Some(c) = gc.chunks.get_mut(idx) else { continue };
            // 0x004ac665..0x004ac73c.
            if c.has_buffers
                && c.coords == [x, y]
                && box_visible(&planes, &camera, c.aabb_min, c.aabb_max, dist + 32.0)
            {
                c.distance_key = dist2_key(c.center, camera.position);
            }
            let mut n = 0i32;
            for p in &c.props {
                let pos = [p.x, p.y, p.z];
                let lit = p.flags & 1 != 0;
                emitters[idx].push(EmitterInput { position: pos, kind: p.kind as i32, emits_light: lit, color: p.f2c });
                // 0x004ac75c..0x004acaab.
                if lit && sphere_visible(&planes, &camera, pos, 16.0, dist) {
                    n += 33;
                    let arg = (clock as f32) * 0.01 + n as f32;
                    let radius = (cw_math::cos(arg as f64) as f32) * 0.5 + 15.0;
                    let mut color = p.f2c;
                    if p.kind == 0xd {
                        let k = 1.0 - daylight * 0.5;
                        color = [k * color[0], k * color[1], k * color[2]];
                    }
                    lights.push(SceneLight { position: pos, radius, color, distance: dist2_key(camera.position, pos) });
                }
                // 0x004acaab: `comiss 4096, key; jbe skip`.
                if 4096.0f32 > c.distance_key
                    && let Some(cue) = ambient_sound(p, tod, clock, dt_ms, rng)
                {
                    sounds.push(cue);
                }
            }
        }
    }
    (lights, emitters)
}

/// `true` when `(phase + clock) / period` differs from `(phase + clock + dt) / period`: the
/// clock crosses a period boundary within this frame. `phase` is
/// `block_y % period + block_x % period` with `block_y = (y + 333·65536) / 65536`, the C
/// `/` and `%` (truncating), as `0x004acaef..0x004acc06` computes it.
fn crosses_boundary(pos: [i64; 3], clock: i32, dt: i32, period: i32) -> bool {
    let bx = (pos[0] / 65536) as i32;
    let by = (pos[1].wrapping_add(0x14d_0000) / 65536) as i32;
    let before = (by - (by / period) * period - (bx / period) * period + bx).wrapping_add(clock) / period;
    let after = (by % period - (bx / period) * period + dt + bx).wrapping_add(clock) / period;
    before != after
}

/// `rand() * 0.2 / 32767 + base` (`0x004acc28..0x004acc45`), the pitch of the ambient sounds.
fn pitch(rng: &mut MsvcRand, base: f32) -> f32 {
    (rng.rand() as f32) * 0.2 / 32767.0 + base
}

/// `0x004acaab..0x004ad40d`: the ambient sound of one prop, if any. Kinds 0x3d, 0x3e, 0x3f
/// are trees, 0..4 grass and flowers.
///
/// - Day (`21 600 000 < t < 75 600 000`): kinds 0x3d/0x3e/0x3f sing bird 0x5e/0x5f/0x60
///   when the clock crosses a 1 s boundary for the prop and `rand() % 10 == 0`; volume 0.1,
///   pitch `rand()·0.2/32767 + 0.9`.
/// - Night: kind 0x3d hoots owl `0x63 + rand() % 2` on the same 1 s test; volume 0.1, pitch
///   `rand()·0.2/32767 + 0.5` (drawn before the id).
/// - Otherwise, from noon to midnight (`43 200 000 < t < 86 400 000`): kinds 2, 3, 4 chirp
///   cricket 0x61, kinds 0, 1 cricket 0x62, on a 5 s boundary and `rand() % 70 == 0`; volume
///   0.05, pitch `rand()·0.2/32767 + 0.9`.
pub fn ambient_sound(p: &Prop, t: i32, clock: i32, dt: i32, rng: &mut MsvcRand) -> Option<SoundCue> {
    let pos = [p.x, p.y, p.z];
    let kind = p.kind as i32;
    let day = 0x149_9700 < t && t < 0x481_9080;
    // The day branch handles 0x3d..0x3f; the night branch only 0x3d; everything else falls
    // to the cricket test (0x004ad0ed).
    if day {
        let id = match kind {
            0x3d => Some(0x5e),
            0x3e => Some(0x5f),
            0x3f => Some(0x60),
            _ => None,
        };
        if let Some(id) = id {
            if crosses_boundary(pos, clock, dt, 1000) && rng.rand() % 10 == 0 {
                let pitch = pitch(rng, 0.9);
                return Some(SoundCue { id, pos, volume: 0.1, pitch });
            }
            return None;
        }
    } else if kind == 0x3d {
        if crosses_boundary(pos, clock, dt, 1000) && rng.rand() % 10 == 0 {
            // 0x004ad099: the pitch argument is evaluated (and its rand drawn) first.
            let pitch = pitch(rng, 0.5);
            let id = 0x63 + (rng.rand() % 2) as u32;
            return Some(SoundCue { id, pos, volume: 0.1, pitch });
        }
        return None;
    }
    // 0x004ad0ed.
    if !(0x293_2e00 < t && t < 0x526_5c00) {
        return None;
    }
    let id = match kind {
        2..=4 => 0x61,
        0 | 1 => 0x62,
        _ => return None,
    };
    if crosses_boundary(pos, clock, dt, 5000) && rng.rand() % 0x46 == 0 {
        let pitch = pitch(rng, 0.9);
        return Some(SoundCue { id, pos, volume: 0.05, pitch });
    }
    None
}

/// `0x004c6cc0(item)`: the average colour of an item's spirit cubes of material `>= 0x80`
/// (fire 0x80, unholy 0x81, ice 0x82, wind 0x83; `0x004c7250` with white and strength 1),
/// all zero when it has none.
pub fn spirit_color(item: &[u8]) -> [f32; 4] {
    let mut sum = [0.0f32; 4];
    let count = i32::from_le_bytes(item[0x114..0x118].try_into().unwrap());
    let mut n = 0;
    for i in 0..count.max(0) as usize {
        let Some(&m) = item.get(0x17 + i * 8) else { break };
        if m <= 0x7f {
            continue;
        }
        n += 1;
        let c = material_color(m, [1.0; 4], 1.0);
        sum = [sum[0] + c[0], c[1] + sum[1], c[2] + sum[2], c[3] + sum[3]];
    }
    if n > 0 {
        let k = 1.0 / n as f32;
        sum = [sum[0] * k, k * sum[1], sum[2] * k, k * sum[3]];
    }
    sum
}

/// `0x004c7250(material, base, strength)`: a material's colour applied to `base`.
pub fn material_color(material: u8, base: [f32; 4], strength: f32) -> [f32; 4] {
    let tint = |k: [f32; 3]| [base[0] * k[0], base[1] * k[1], base[2] * k[2], base[3]];
    match material {
        1 => tint([0.7, 0.7, 0.7]),
        2 => tint([0.4, 0.3, 0.2]),
        5 => tint([0.1, 0.1, 0.1]),
        7 => tint([0.9, 0.9, 0.9]),
        0xb => [base[0], base[1] * 0.7, base[2] * 0.2, base[3]],
        0xc => tint([0.8, 0.8, 0.85]),
        0x80..=0x83 => {
            let f = strength * 0.5 + 1.0;
            let a = (base[3] + strength) * f;
            match material {
                0x80 => [f, f * 0.5, f * 0.1, a],
                0x81 => [f * 0.3, f, f * 0.5, a],
                0x82 => [f * 0.3, f * 0.5, f, a],
                _ => [f * 0.8, f * 0.8, f, a],
            }
        }
        _ => tint([0.5, 0.5, 0.5]),
    }
}

/// `0x004c71c0(item)`: the glow of a ground item (a type-0xb sub-0x13 item, a type-0x12
/// sub-0 or sub-1 item), zero for the rest.
pub fn item_glow(item: &Item) -> [f32; 3] {
    match (item.item_type, item.sub_type) {
        (0xb, 0x13) => [0.1, 0.3, 0.3],
        (0x12, 0) => [0.6, 0.6, 0.2],
        (0x12, 1) => [0.0, 0.6, 0.2],
        _ => [0.0; 3],
    }
}

fn f32_at(e: &EntityData, o: usize) -> f32 {
    f32::from_le_bytes(e.0[o..o + 4].try_into().unwrap())
}
fn i32_at(e: &EntityData, o: usize) -> i32 {
    i32::from_le_bytes(e.0[o..o + 4].try_into().unwrap())
}
fn i64_at(e: &EntityData, o: usize) -> i64 {
    i64::from_le_bytes(e.0[o..o + 8].try_into().unwrap())
}
fn u16_at(e: &EntityData, o: usize) -> u16 {
    u16::from_le_bytes(e.0[o..o + 2].try_into().unwrap())
}

/// Entity position (`entity+0x00`).
pub fn entity_pos(e: &EntityData) -> [i64; 3] {
    [i64_at(e, 0), i64_at(e, 8), i64_at(e, 0x10)]
}

/// Equipment slot `n` of an entity (`entity+0x2f0 + n·0x118`).
pub fn equip(e: &EntityData, slot: usize) -> &[u8] {
    let o = 0x2f0 + slot * 0x118;
    &e.0[o..o + 0x118]
}

fn light(position: [i64; 3], radius: f32, color: [f32; 3], camera: [i64; 3]) -> SceneLight {
    SceneLight { position, radius, color, distance: dist2_key(camera, position) }
}

/// `0x004af353..0x004b0e84`: the dynamic lights, in the original's order, then (with the
/// emitter lights of [`chunk_walk`] in front) `std::sort` by distance, near first
/// (`0x004abae0`, VC11 introsort: ties keep the original's arrangement).
pub fn gather_lights(ctx: &SceneCtx, emitter_lights: Vec<SceneLight>, timer: &mut i32) -> (Vec<SceneLight>, Vec<PointLightSource>) {
    let gc = ctx.gc;
    let cam = gc.camera.position;
    let mut dynamic = Vec::new();
    // 0x004af353..0x004af486: projectiles of kind 1 (fireballs): (0.5, 0.5, 0.2) × `+0x4c`,
    // radius 16.
    for p in ctx.projectiles {
        if p.kind == 1 {
            let r = p.radius;
            dynamic.push(light(p.pos, 16.0, [0.5 * r, 0.5 * r, 0.2 * r], cam));
        }
    }
    // 0x004af486..0x004af5c5: flash lights, colour × max(0, 1 - age / duration).
    for f in &gc.flash_lights {
        let mut k = 1.0 - (f.age as f32) / (f.duration as f32);
        if 0.0 > k {
            k = 0.0;
        }
        dynamic.push(light(f.position, f.radius, [f.color[0] * k, f.color[1] * k, f.color[2] * k], cam));
    }
    // 0x004af5c5..0x004b06f3: the creatures, in map order.
    for (&id, e) in ctx.entities {
        creature_lights(ctx, id, e, &mut dynamic);
    }
    // 0x004b06f3..0x004b0756: the 100 ms timer.
    *timer = timer.wrapping_add(ctx.dt_ms);
    if *timer > 100 {
        *timer = 0;
    }
    // 0x004b0756..0x004b0e42: zones around the local player.
    zone_lights(ctx, &mut dynamic);
    let sources = dynamic.iter().map(SceneLight::source).collect();
    let mut all = emitter_lights;
    all.extend(dynamic);
    cw_math::sort::msvc_sort(&mut all, |a, b| a.distance < b.distance);
    (all, sources)
}

/// `0x004af602..0x004b06b3`: the lights one living creature carries.
fn creature_lights(ctx: &SceneCtx, id: i64, e: &EntityData, out: &mut Vec<SceneLight>) {
    let cam = ctx.gc.camera.position;
    // 0x004af622: `comiss 0, hp; jae skip` (NaN passes).
    if 0.0 >= f32_at(e, 0x15c) {
        return;
    }
    let st = ctx.states.get(&id);
    let render_pos = st.map_or(entity_pos(e), |s| s.riding.render_pos);
    let render_rot = st.map_or([0.0; 3], |s| s.riding.render_rot);
    let height = f32_at(e, 0x78);
    let yaw = f32_at(e, 0x20);
    // The hand offset of torches, lamps and buff 9: Rz(yaw) · (0.5, 1, height·0.3).
    let hand = || {
        let mut m = identity();
        pre_rotate_z(&mut m, yaw);
        let p = transform_point(&m, [0.5, 1.0, height * 0.3]);
        add3(render_pos, fixed_from_float(p))
    };
    // 0x004af632: a torch (weapon sub type 0x14) in slot 7 or 6.
    if equip(e, 7)[1] == 0x14 || equip(e, 6)[1] == 0x14 {
        out.push(light(hand(), 32.0, [1.2, 1.0, 0.5], cam));
    }
    // 0x004af78d: the lamp (flag 0x200 and a type-0x18 item in slot 10): radius
    // rarity·8 + 10, colour (0.6, 0.5, 0.4) × (rarity·0.25 + 0.75).
    let lamp = equip(e, 10);
    if u16_at(e, 0x114) & 0x200 != 0 && lamp[0] == 0x18 {
        let r = lamp[0xc];
        let k = f32::from(r) * 0.25 + 0.75;
        out.push(light(hand(), (i32::from(r) * 8 + 10) as f32, [0.6 * k, 0.5 * k, 0.4 * k], cam));
    }
    // 0x004af937: buff 9.
    if st.is_some_and(|s| cw_sim::util::has_buff(s, 9)) {
        out.push(light(hand(), 16.0, [0.6, 0.4, 0.0], cam));
    }
    // 0x004afa8c..0x004afeec: glowing creature types.
    let ty = i32_at(e, 0x54);
    let raised = || add3(entity_pos(e), fixed_from_float([0.0, 0.0, 1.5]));
    match ty {
        0x75 => out.push(light(render_pos, 16.0, [0.0, 0.4, 0.8], cam)),
        0x87 => out.push(light(raised(), 16.0, [0.1, 0.5, 0.1], cam)),
        0x88 => out.push(light(raised(), 16.0, [0.1, 0.1, 0.5], cam)),
        0x89 => out.push(light(raised(), 16.0, [0.5, 0.1, 0.2], cam)),
        0x8a => out.push(light(raised(), 16.0, [0.8, 0.8, 0.6], cam)),
        _ => {}
    }
    // 0x004afeec: appearance flag 0x200: `0x004460f0` colour × 0.5 ((0.8, 0, 0.5) when the
    // hostility byte is 1, white otherwise).
    if u16_at(e, 0x6e) & 0x200 != 0 {
        let c = if e.0[0x50] == 1 { [0.8, 0.0, 0.5] } else { [1.0, 1.0, 1.0] };
        out.push(light(render_pos, 16.0, [c[0] * 0.5, c[1] * 0.5, c[2] * 0.5], cam));
    }
    // 0x004affa9..0x004b0184: casting glow in front of the creature.
    let mode = e.0[0x58];
    let mode_time = i32_at(e, 0x5c);
    let casting = match mode {
        0x1c | 0x5f | 0x5e | 0x24 => true,
        0x57 | 0x58 | 0x59 | 0x5c | 0x25 | 0x26 | 0x2b | 0x2c => {
            let (guard, haste) = st.map_or((0.0, false), |s| (s.block, cw_sim::util::has_buff(s, 0xc)));
            mode_time < cw_sim::skills::skill_windup(e, guard, haste, -1)
        }
        _ => false,
    };
    if casting {
        let mut m = identity();
        pre_rotate_z(&mut m, render_rot[2]);
        let v = transform_vector(&m, [0.0, 1.0, 0.0]);
        let pos = add3(render_pos, fixed_from_float(v));
        // `0x0040e420`: (float)cos((double)x).
        let c = cosf((mode_time as f32) * 0.01);
        let k = c * 0.1 + 1.0;
        out.push(light(pos, 20.0, [0.3 * k, 0.2 * k, 0.1 * k], cam));
    }
    // 0x004b0184: the charge `entity+0x60 / maxBlock`; nothing more at exactly 0.
    let charge = (i32_at(e, 0x60) as f32) / (cw_sim::skills::max_block(e) as f32);
    if charge == 0.0 {
        return;
    }
    let pose = ctx.gc.poses.get(&id).copied().unwrap_or_default();
    // 0x004b01fe..0x004b0286: fixed(tip) - (origin x, origin y, 0), exactly as the original.
    // The stored tips are last frame's, already moved by -(render_pos + origin) in x/y by the
    // pose (0x004225fd), so this lands near `-origin` (about the camera) plus the tip's offset
    // from the creature, not at the weapon: an original quirk, kept.
    let tip = |t: [f32; 3]| {
        let f = fixed_from_float(t);
        add3(f, [pose.origin[0].wrapping_neg(), pose.origin[1].wrapping_neg(), 0])
    };
    let glow = |slot: usize, k: f32| {
        let c = spirit_color(equip(e, slot));
        if c[3] > 0.0 { Some([(c[0] * charge) * k, (c[1] * charge) * k, (c[2] * charge) * k]) } else { None }
    };
    // 0x004b01c1: a weapon in slot 7, at the right tip.
    if equip(e, 7)[0] == 3
        && let Some(c) = glow(7, 0.3)
    {
        out.push(light(tip(pose.right_tip()), 10.0, c, cam));
    }
    // 0x004b033e: a weapon in slot 6, at the left tip.
    if equip(e, 6)[0] == 3
        && let Some(c) = glow(6, 0.5)
    {
        out.push(light(tip(pose.left_tip()), 10.0, c, cam));
    }
    // 0x004b04bb, 0x004b05b7: slots 5 and 2 at the entity position.
    if equip(e, 5)[0] != 0
        && let Some(c) = glow(5, 0.1)
    {
        out.push(light(entity_pos(e), 10.0, c, cam));
    }
    if equip(e, 2)[0] != 0
        && let Some(c) = glow(2, 0.1)
    {
        out.push(light(entity_pos(e), 10.0, c, cam));
    }
}

/// The zones `[x0, x1] × [y0, y1]` within `range` blocks of the local player
/// (`0x004b0756..0x004b0835`): `trunc((block ∓ range) × (1/256))` per axis.
pub fn zones_around(pos: [i64; 3], range: f32) -> ([i32; 2], [i32; 2]) {
    let bx = (pos[0] / 65536) as i32;
    let by = (pos[1] / 65536) as i32;
    let z = |b: i32, s: f32| (((b as f32) + s) * 0.003_906_25) as i32;
    ([z(bx, -range), z(bx, range)], [z(by, -range), z(by, range)])
}

/// `0x004b0835..0x004b0e42`: statics 0x32 (lamp posts), 0x33 and 0x41 (fires) and glowing
/// ground items of the zones around the local player. Uses this frame's planes.
fn zone_lights(ctx: &SceneCtx, out: &mut Vec<SceneLight>) {
    let gc = ctx.gc;
    let cam = gc.camera.position;
    let Some(player) = ctx.entities.get(&gc.player) else { return };
    let (xs, ys) = zones_around(entity_pos(player), ctx.draw_distance);
    let night = 1.0 - ctx.daylight;
    for zx in xs[0]..=xs[1] {
        for zy in ys[0]..=ys[1] {
            let Some(zone) = ctx.world.zone(zx, zy) else { continue };
            for s in &zone.statics {
                let pos = [s.x, s.y, s.z];
                let top = add3(pos, fixed_from_float([0.0, 0.0, s.scale[2]]));
                // 0x004b08c3: kind 0x32, tested at its top, margin 20.
                if s.kind == 0x32 && sphere_visible(&ctx.planes, &gc.camera, top, 20.0, ctx.draw_distance) {
                    out.push(light(top, 20.0, [1.2 * night, 1.0 * night, 0.8 * night], cam));
                }
                // 0x004b0a6f: kinds 0x33 and 0x41, tested at their base, margin 32.
                if (s.kind == 0x33 || s.kind == 0x41) && sphere_visible(&ctx.planes, &gc.camera, pos, 32.0, ctx.draw_distance)
                {
                    out.push(light(top, 20.0, [1.2 * night, 0.8 * night, 0.4 * night], cam));
                }
            }
            // 0x004b0c0d: ground items not yet claimed (`+0x140 == 0`), 3 blocks up, margin 16,
            // within the props' far distance.
            for it in &zone.items {
                if it.f140 != 0 {
                    continue;
                }
                let c = item_glow(&it.item);
                if !(length_sq(c) > 0.0) {
                    continue;
                }
                let pos = [it.x, it.y, it.z.wrapping_add(3 << 16)];
                if sphere_visible(&ctx.planes, &gc.camera, pos, 16.0, ctx.prop_far) {
                    out.push(light(pos, 16.0, c, cam));
                }
            }
        }
    }
}

/// `0x004b1a6b..0x004b21ac`, the chunk loop of the terrain pass: the visible chunks near →
/// far; for those within `prop_far + 32` blocks horizontally, every prop in list order:
///
/// - kind 0x3c (a tree): when the clock crosses a second boundary this frame and
///   `rand() % 16 == 0`, a leaf falls ([`leaf_particle`]);
/// - otherwise a prop with a model (kind below the table size, not 0x12) whose sphere
///   (centre raised by half its height, margin `max(size x, size z) · scale`) is visible
///   within `prop_far` is drawn now ([`draw_prop`], alpha 1) when its squared distance is at
///   most `prop_near²`, else goes to the far list.
///
/// Nothing is drawn (and no `rand()` drawn) when the frame is white-cleared (`0x004b1af4`
/// clears the visible list).
pub fn terrain_props(
    ctx: &SceneCtx,
    visible: &[usize],
    particles: &mut ParticleSystem,
    rng: &mut MsvcRand,
    far: &mut Vec<FarEntry>,
) -> Vec<Vec<ModelDraw>> {
    let gc = ctx.gc;
    let mut out = vec![Vec::new(); gc.chunks.len()];
    let cam = gc.camera.position;
    let range = ctx.prop_far + 32.0;
    for &idx in visible {
        let c = &gc.chunks[idx];
        // 0x004b1dde: horizontal distance of the chunk centre.
        let d = float_from_fixed(sub3(c.center, cam));
        if length_sq2([d[0], d[1]]) > range * range {
            continue;
        }
        let mut index = 0i32;
        for p in &c.props {
            index += 1;
            if p.kind == 0x3c {
                // 0x004b1ebd: the world clock's second boundary (not per prop).
                let clock = gc.clock_ms;
                if (clock.wrapping_add(ctx.dt_ms)) / 1000 != clock / 1000 && rng.rand() % 16 == 0 {
                    particles.push(leaf_particle([p.x, p.y, p.z], p.f2c, rng));
                }
                continue;
            }
            // 0x004b25af.
            if (p.kind as usize) >= ctx.models.prop_model_count() || p.kind == 0x12 {
                continue;
            }
            let Some(model) = ctx.models.prop_model(p.kind) else { continue };
            let [sx, _, sz] = model.size;
            let margin = (if sx > sz { sx } else { sz }) as f32 * p.scale;
            let half = [0, 0, ftol((sz as f32) * 0.5 * p.scale * 65536.0)];
            let centre = add3([p.x, p.y, p.z], half);
            if !sphere_visible(&ctx.planes, &gc.camera, centre, margin, ctx.prop_far) {
                continue;
            }
            let dist2 = length_sq(float_from_fixed(sub3(centre, cam)));
            // 0x004b2767: `comiss dist2, near²; jbe near`.
            if dist2 > ctx.prop_near * ctx.prop_near {
                far.push(FarEntry { object: FarObject::Prop { prop: p.clone(), index }, dist2 });
            } else if let Some(d) = draw_prop(ctx, p, model, dist2, ctx.prop_far, ctx.prop_near, index) {
                out[idx].push(d);
            }
        }
    }
    out
}

/// `0x004bd160(prop, dist2, far, near, index, …, material)`: one prop.
///
/// World = `T(pos) · S(scale) · [sway] · Rz(rotation) · T(-size x / 2, -size y / 2, 0)`
/// (read left to right as applied to the model); alpha 1 within `near`, fading linearly to 0
/// at `far`; material = the world material with alpha × `f28 / 255` (the prop's light)
/// unless `flags & 1` (lit props are full bright).
///
/// Props with `flags & 4` sway in the wind ([`sway`]): two shears, the first by
/// `(cos(clock·0.004 + 30i + 84) + cos(clock·0.0027 + 93)) · 0.1 / (size z · scale)`, the
/// second by `(cos(clock·0.005 + 30i) + cos(clock·0.003)) · 0.1 / (size z · scale) · 0.8`.
pub fn draw_prop(ctx: &SceneCtx, p: &Prop, model: ModelInfo, dist2: f32, far: f32, near: f32, index: i32) -> Option<ModelDraw> {
    let gc = ctx.gc;
    let mut m = identity();
    pre_translate(&mut m, render_translation([p.x, p.y, p.z], gc.camera.render_offset));
    pre_scale(&mut m, [p.scale, p.scale, p.scale]);
    if p.flags & 4 != 0 {
        // 0x004bd507: `0.1 / (size z · scale)`.
        let k = 0.1 / ((model.size[2] as f32) * p.scale);
        let clock = gc.clock_ms as f32;
        // 0x004bd643: `index · 30` in integers (`(i << 4) - i` doubled).
        let phase = index.wrapping_mul(30) as f32;
        let a = (cosf(clock * 0.004 + phase + 84.0) + cosf(clock * 0.0027 + 93.0)) * k;
        let b = (cosf(clock * 0.005 + phase) + cosf(clock * 0.003)) * k * 0.8;
        sway(&mut m, a, b);
    }
    pre_rotate_z(&mut m, p.rotation);
    pre_translate(&mut m, [(model.size[0] as f32) * -0.5, (model.size[1] as f32) * -0.5, 0.0]);
    // 0x004be62c: the fade (`comiss dist2, near²; jbe` → 1, so NaN is opaque).
    let alpha = if !(dist2 > near * near) {
        1.0
    } else {
        1.0 - (((dist2 as f64).sqrt() as f32) - near) / (far - near)
    };
    let wm = ctx.world_material;
    let a = if p.flags & 1 == 0 { (p.f28 / 255.0) * wm[3] } else { 1.0 };
    Some(ModelDraw {
        origin: 0x004b_d160,
        model: model.handle,
        world: m,
        material: [wm[0], wm[1], wm[2], a],
        alpha,
        double_sided: false,
        shininess: 0.0,
        set_white: None,
        mirrored: false,
    })
}

/// The wind sway of `0x004bd160` (`0x004bd507..0x004be40f`, inlined matrix code): with
/// `(c1, s1)` = cos/sin of the double `-1.5707963705062866` and `(c2, s2)` of `+1.57…`
/// (`0x00702ad0`, radians, not degrees), rows `r0..r3` of `M`:
///
/// 1. rows 0/2 by θ1: `r2 = r2·c1 + r0·s1`, `r0 = r0·c1 - r2·s1`;
/// 2. shear: `r0 = r0 + a·r1` (a product with an identity whose `[0][1]` is `a`);
/// 3. rows 0/2 by θ2: `r0 = r0·c2 - r2·s2`, `r2 = r0·s2 + r2·c2`;
/// 4. rows 1/2 by θ1: `r2 = r2·c1 - s1·r1`, `r1 = r1·c1 + r2·s1`;
/// 5. shear: `r1 = r1 + b·r0`;
/// 6. rows 1/2 by θ2: `r2 = r2·c2 - r1·s2`, `r1 = r2·s2 + r1·c2`.
///
/// The zero terms of the two identity products (`0·x`) are left out: they add exact zeros.
fn sway(m: &mut D3dMatrix, a: f32, b: f32) {
    let t1 = -1.570_796_370_506_286_6f64;
    let t2 = 1.570_796_370_506_286_6f64;
    let (c1, s1) = (cw_math::cos(t1) as f32, cw_math::sin(t1) as f32);
    let (c2, s2) = (cw_math::cos(t2) as f32, cw_math::sin(t2) as f32);
    for j in 0..4 {
        let (r0, r2) = (m[0][j], m[2][j]);
        m[2][j] = r2 * c1 + r0 * s1;
        m[0][j] = r0 * c1 - r2 * s1;
    }
    for j in 0..4 {
        m[0][j] += a * m[1][j];
    }
    for j in 0..4 {
        let (r0, r2) = (m[0][j], m[2][j]);
        m[0][j] = r0 * c2 - r2 * s2;
        m[2][j] = r0 * s2 + r2 * c2;
    }
    for j in 0..4 {
        let (r1, r2) = (m[1][j], m[2][j]);
        m[2][j] = r2 * c1 - s1 * r1;
        m[1][j] = r1 * c1 + r2 * s1;
    }
    for j in 0..4 {
        m[1][j] += b * m[0][j];
    }
    for j in 0..4 {
        let (r1, r2) = (m[1][j], m[2][j]);
        m[2][j] = r2 * c2 - r1 * s2;
        m[1][j] = r2 * s2 + r1 * c2;
    }
}

/// `0x004ba462..0x004ba5d0`: the far list, sorted far → near (`0x004abac0`, predicate
/// `a.dist2 > b.dist2`), drawn after the water with the fade: props between `prop_near` and
/// `prop_far`, statics between `static_near` and `static_far`.
pub fn far_draws(ctx: &SceneCtx, far: &mut [FarEntry]) -> Vec<ModelDraw> {
    cw_math::sort::msvc_sort(far, |a, b| a.dist2 > b.dist2);
    let mut out = Vec::new();
    for f in far.iter() {
        match &f.object {
            FarObject::Prop { prop, index } => {
                if let Some(model) = ctx.models.prop_model(prop.kind)
                    && let Some(d) = draw_prop(ctx, prop, model, f.dist2, ctx.prop_far, ctx.prop_near, *index)
                {
                    out.push(d);
                }
            }
            FarObject::Static { zone, index } => {
                if let Some(z) = ctx.world.zone(zone.0, zone.1)
                    && let Some(s) = z.statics.get(*index)
                {
                    out.extend(statics::draw_static(ctx, s, *zone, *index, f.dist2, ctx.static_far, ctx.static_near));
                }
            }
        }
    }
    out
}

/// The visible chunks of this frame near → far (`0x004ac665..0x004ac73c` with the previous
/// frame's planes, sorted by `0x004aba70`): ring indices.
pub fn visible_chunks(gc: &SceneState) -> Vec<usize> {
    let w = gc.window;
    let mut out: Vec<(usize, f32)> = Vec::new();
    if w.size <= 0 {
        return Vec::new();
    }
    let dist = draw_distance(&gc.camera) + 32.0;
    for x in w.origin[0]..w.origin[0] + w.size {
        for y in w.origin[1]..w.origin[1] + w.size {
            if x < 0 || y < 0 || x >= 0x80000 || y >= 0x80000 {
                continue;
            }
            let idx = ring_index(&w, x, y);
            let Some(c) = gc.chunks.get(idx) else { continue };
            if !c.has_buffers || c.coords != [x, y] {
                continue;
            }
            if box_visible(&gc.previous_frustum, &gc.camera, c.aabb_min, c.aabb_max, dist) {
                out.push((idx, c.distance_key));
            }
        }
    }
    cw_math::sort::msvc_sort(&mut out, |a, b| a.1 < b.1);
    out.into_iter().map(|(i, _)| i).collect()
}

/// Gather one world frame (`GameController::render` minus the device work), in the
/// original's order. `dt_ms` is the frame time (`GC+0x8006e8`); `rng` is the global `rand()`
/// stream shared with the client's `World::tick`.
///
/// `gc` is mutable for the three things `render` writes back: the chunk distance keys, the
/// timer `0x0076b17c` and the GUI visibility; `particles` receives the leaves, explosions
/// and beam sparks.
#[allow(clippy::too_many_arguments)]
pub fn gather_scene(
    world: &World,
    entities: &BTreeMap<i64, EntityData>,
    states: &BTreeMap<i64, CreatureState>,
    projectiles: &[cw_sim::projectile::Projectile],
    gc: &mut SceneState,
    particles: &mut ParticleSystem,
    models: &dyn SceneModels,
    dt_ms: i32,
    rng: &mut MsvcRand,
) -> (ScenePieces, Vec<SoundCue>) {
    let mut sounds = Vec::new();
    let mut out = ScenePieces::default();
    let d = daylight(gc.time_of_day_ms);
    // 0x004ac4c7..0x004ad434.
    let (emitter_lights, emitters) = chunk_walk(gc, d, dt_ms, rng, &mut sounds);
    out.chunk_emitters = emitters;
    match gc.mode {
        // 0x004ad43a..0x004ad67f: the loading screen draws the GUI only.
        cw_render::passes::FrameMode::GuiOnly => {
            out.gui = gc.gui;
            return (out, sounds);
        }
        // 0x004adede..0x004ae279: the map screen: the map, then its compass (0x004ae21b).
        cw_render::passes::FrameMode::MapScreen => {
            out.gui = gc.gui;
            let ctx = minimal_ctx(world, entities, states, projectiles, gc, models, d, dt_ms);
            out.map_compass = hud::map_compass(&ctx);
            return (out, sounds);
        }
        cw_render::passes::FrameMode::World => {}
    }
    // 0x004ae27e..0x004ae370.
    gc.gui.apply();
    out.gui = gc.gui;
    // 0x004baa95/0x004bb080: the minimap and HUD models also need the engine's root widget.
    out.hud_visible = gc.gui.hud && gc.engine_root_visible;
    // 0x004ad6e2: the camera block (water: underwater tint).
    let cb = cw_render::light::get_block(
        world,
        block_of_fixed(gc.camera.position[0]),
        block_of_fixed(gc.camera.position[1]),
        block_of_fixed(gc.camera.position[2]),
    );
    let underwater = cb[3] & 0x1f == 2;
    // 0x004aee2e..0x004af353: this frame's planes.
    let proj = cw_render::passes::projection(gc.screen, gc.camera.narrow_fov);
    let planes = cw_render::passes::frustum_planes(&gc.camera.view, &proj);
    let dd = draw_distance(&gc.camera);
    let prop_far = gc.view_distance * 0.2;
    let mut timer = gc.timer_76b17c;
    let visible = if gc.white_clear { Vec::new() } else { visible_chunks(gc) };
    let ctx = SceneCtx {
        world,
        entities,
        states,
        gc,
        models,
        projectiles,
        planes,
        daylight: d,
        underwater,
        world_material: cw_render::passes::world_material(d, underwater),
        draw_distance: dd,
        prop_far,
        prop_near: prop_far * 0.9,
        static_far: dd * 0.3,
        static_near: dd * 0.3 * 0.9,
        particle_range: dd * 0.2,
        dt_ms,
    };
    // 0x004af353..0x004b0e84.
    let (lights, dynamic) = gather_lights(&ctx, emitter_lights, &mut timer);
    out.lights = lights;
    out.dynamic_lights = dynamic;
    // 0x004b1a6b..0x004b21ac.
    let mut far = Vec::new();
    out.chunk_props = terrain_props(&ctx, &visible, particles, rng, &mut far);
    // 0x004b21ac..0x004b32ed: ground items and statics (skipped when white-cleared).
    if !gc.white_clear {
        statics::zone_objects(&ctx, particles, rng, &mut far, &mut out.objects);
    }
    // 0x004b3338: the target marker.
    if gc.target_marker {
        out.objects.extend(items::target_marker(&ctx));
    }
    // 0x004b35d4..0x004b7db6.
    let discovered = |zx: i32, zy: i32| gc.zone_discovered.get(&(zx, zy)).copied();
    let inputs = creatures::PassInputs {
        parts: &gc.pose_parts,
        explode_pending: &gc.explode_pending,
        real_time_ms: gc.frame_time_ms,
        zone_discovered: &discovered,
        show_paths: gc.show_paths,
        airships: &gc.airships,
    };
    let mut effects = creatures::PassEffects::default();
    creatures::creature_pass_with(&ctx, &inputs, particles, rng, &mut out, &mut effects);
    // 0x004b7db6..0x004b8138: particles, double-sided.
    out.creature_extras.extend(particles.draw(
        &gc.camera,
        &planes,
        ctx.particle_range,
        ctx.world_material,
        &|b| cw_render::light::prop_light(cw_render::light::get_block(world, b[0], b[1], b[2])),
        models.particle_cube(),
    ));
    // 0x004b8138..0x004b8bb9: projectiles, after the particles.
    creatures::projectile_pass(&ctx, particles, rng, &mut out);
    // 0x004b8bc7..0x004ba13e.
    out.shadows = fx::shadows(&ctx);
    out.ribbons = fx::ribbons(&ctx);
    // 0x004ba462..0x004ba5d0.
    out.far_objects = far_draws(&ctx, &mut far);
    // 0x004baa67.
    out.clear_overlay_nodes = true;
    // 0x004bb080..0x004bbb1a.
    let hud_visible = out.hud_visible;
    if hud_visible {
        out.hud_models = hud::hud_models(&ctx);
    }
    gc.timer_76b17c = timer;
    // The creature pass's writes: flash lights pushed to `GC+0x800748` (0x004b4118) and the
    // `creature+0x1d10` flags, cleared (0x004b411d, 0x004b4129) and set again by each pose
    // draw (0x0041293b).
    effects.apply_flags(&mut gc.explode_pending);
    gc.flash_lights.extend(effects.flash_lights);
    out.body_materials = effects.body_materials;
    // 0x004bbabc..0x004bbb16: the item preview keeps turning while the HUD shows.
    if hud_visible {
        gc.item_preview_rotation = hud::item_preview_spin(gc.item_preview_rotation, dt_ms);
    }
    (out, sounds)
}

/// A [`SceneCtx`] for the frame shapes that stop early (the map screen's compass).
#[allow(clippy::too_many_arguments)]
fn minimal_ctx<'a>(
    world: &'a World,
    entities: &'a BTreeMap<i64, EntityData>,
    states: &'a BTreeMap<i64, CreatureState>,
    projectiles: &'a [cw_sim::projectile::Projectile],
    gc: &'a SceneState,
    models: &'a dyn SceneModels,
    daylight: f32,
    dt_ms: i32,
) -> SceneCtx<'a> {
    let dd = draw_distance(&gc.camera);
    let prop_far = gc.view_distance * 0.2;
    SceneCtx {
        world,
        entities,
        states,
        gc,
        models,
        projectiles,
        planes: gc.previous_frustum,
        daylight,
        underwater: false,
        world_material: cw_render::passes::world_material(daylight, false),
        draw_distance: dd,
        prop_far,
        prop_near: prop_far * 0.9,
        static_far: dd * 0.3,
        static_near: dd * 0.3 * 0.9,
        particle_range: dd * 0.2,
        dt_ms,
    }
}

#[cfg(test)]
mod tests;
