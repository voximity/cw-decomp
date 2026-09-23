//! `GameController::render` (`Cube.exe 0x004ac260`, 59 KB, the only BeginScene/EndScene
//! caller) as a pure function from what the controller hands over ([`RenderInputs`]) to a
//! backend-independent frame description ([`FrameCommands`]).
//!
//! Fidelity: Tier B for the order of passes and for every number fed to the shaders
//! (daylight, sun direction, sky/fog/light colours from `0x0059d640`, fog scale and
//! gradient, point-light selection and assignment, cloud placement, star and sun sprites,
//! culling); Tier C for the state machinery (D3D9 render states become [`PipelineState`]).
//!
//! # Map of the original
//!
//! `render` is one straight-line function. The pieces below are split by responsibility;
//! each names its address range. Ranges not ported here belong to other modules (pose,
//! GUI) or are listed under "Not ported" and in the module report.
//!
//! | Range | Piece | Here |
//! |---|---|---|
//! | `0x004ac260..0x004ac312` | prologue, `timeGetTime`, recolours a GUI object at `GC+0x304` element `0xa08` to (0,100,200) | not ported (GUI side) |
//! | `0x004ac312..0x004ac392` | `Clear(TARGET\|ZBUFFER, 0xff0070ff or white, 1.0, 0)`, `ZENABLE=1`, `ZFUNC=LESSEQUAL`, `World::lock`, `BeginScene` | [`PassKind::ClearFrame`] |
//! | `0x004ac394..0x004ac46e` | daylight `d = (1 - (2t/86.4e6 - 1)^4)^5` | [`daylight`] |
//! | `0x004ac475..0x004ac4c7` | bump a use counter of every dirty chunk (`0x00468910`) | not ported (cache bookkeeping) |
//! | `0x004ac4c7..0x004ad434` | chunk window walk: rebuild stale chunk buffer lists (`0x004ab940`, `0x004570a0`), frustum/distance test (`0x0047f3c0`) and distance key (`+0x6c`), light-emitting statics → point lights, ambient sounds (`playSound` bird/cricket/owl with `rand()`) | [`collect_visible_chunks`]; emitter lights and sounds: the gatherer (`cw_client::scene::chunk_walk`) |
//! | `0x004ad43a..0x004ad67f` | loading/start screen: GUI only, panels hidden, `EndScene`, return | [`build_gui_only`] |
//! | `0x004ad684..0x004adb04` | CubeShader constants: material (underwater tint), white, alpha, sky colours `0x0059d640`, sun direction, `CULLMODE=NONE`, camera position, light, darkness, shininess, fog scale, fog gradient, zero point lights | [`frame_uniforms`], [`sky_colors`] |
//! | `0x004adb04..0x004adede` | sky fan: 4 `CubeVertex`, transforms, VS00+PS02, `ZENABLE=0`, `DrawPrimitiveUP(FAN, 2)` unless white | [`PassKind::Sky`] |
//! | `0x004adede..0x004ae279` | map screen (`GC+0x8006e4`): `0x005fc1b0`, lights, depth clear, compass `0x00476660`, GUI, `EndScene`, return | [`build_map_screen`] |
//! | `0x004ae27e..0x004ae370` | HUD visibility flags | not ported (GUI side) |
//! | `0x004ae370..0x004ae641` | `LIGHTING=0`, FVF `0x42`, VS/PS NULL, star matrix, projection `0x00427910`, `SetTexture(0,NULL)`, fixed-function transforms | [`projection`], [`star_matrix`] |
//! | `0x004ae641..0x004aea83` | stars when `d < 0.5` and not underwater | [`PassKind::Stars`] |
//! | `0x004aea83..0x004aed63` | sun sprite (FVF `0x142`, texture `GC+0x8006e0`, clamp) | [`PassKind::Sun`] |
//! | `0x004aed63..0x004aee2e` | bind VS00+PS01, sky colours, save projection (`GC+0x800a90`), `ZENABLE=1`, alpha blend `SRCALPHA/INVSRCALPHA` | [`PassKind::Clouds`] start state |
//! | `0x004aee2e..0x004af353` | frustum planes from view·projection → `GC+0x1000fa4` | [`frustum_planes`] |
//! | `0x004af353..0x004b0e84` | dynamic point lights (world list `+0x2f8`, particles `+0x800748`, lamp-carrying creatures, …) | the gatherer; input [`RenderInputs::lights`] (emitters and dynamic lights, sorted near → far by `std::sort 0x004abae0`) |
//! | `0x004b0e84..0x004b12d7` | zero every chunk's light slots, assign lights to the chunks they touch (≤ 16 each) | [`assign_lights_to_chunks`] |
//! | `0x004b12d7..0x004b142f` | global set = first min(n, 16) lights | [`FrameCommands::light_sets`] |
//! | `0x004b142f..0x004b1a6b` | clouds: VS00+PS05, darkness, fog×1.5, 13×13 grid | [`cloud_draws`] |
//! | `0x004b1a6b..0x004b21ac` | terrain: lights, fog, darkness, sort visible chunks near→far, `CULLMODE=CW`, per chunk: material, chunk lights, alpha, each buffer's opaque indices; fire smoke (`rand`) and near/far static props | [`PassKind::Terrain`] |
//! | `0x004b21ac..0x004b35d4` | global lights; zone objects, dropped items, props; target marker | [`PassKind::Objects`] |
//! | `0x004b35d4..0x004b8bc7` | creatures (flash, after-images, ghost deferral, pose `0x004128f0`), equipment, projectiles, skill effects | [`PassKind::Creatures`] |
//! | `0x004b8bc7..0x004b9993` | blob shadows: fixed function FVF `0x142`, texture `GC+0x8006dc`, `ZWRITE=0`, clamp | [`PassKind::Shadows`] |
//! | `0x004b9993..0x004ba13e` | ribbons: FVF `0x42`, `CULLMODE=NONE`, `TRIANGLESTRIP` of 30 | [`PassKind::Ribbons`] |
//! | `0x004ba13e..0x004ba462` | water: alpha 1, VS00+PS03, `ZENABLE=1`, `ZWRITE=1`, `CULLMODE=NONE`, visible chunks far→near, second index buffer; then `ZWRITE=1`, `CULLMODE=CW`, VS00+PS01 | [`PassKind::Water`] |
//! | `0x004ba462..0x004ba5d0` | the far list (`0x004bd160` props, `0x004be760` statics), far → near, faded | [`PassKind::FarObjects`], input [`RenderInputs::far_objects`] |
//! | `0x004ba5d0..0x004ba9fc` | ghosted creatures: `COLORWRITEENABLE=0` draw, `=0xF` draw with alpha `1 - 0.75·ghost` | [`PassKind::Ghosts`] |
//! | `0x004ba9fc..0x004baa95` | white 0, bind, alpha 1, material 1, `0x00632870`, `Clear(ZBUFFER)` | [`PassKind::ClearDepth`] |
//! | `0x004baa95..0x004bb075` | minimap `0x005fc1b0` and markers | [`PassKind::Minimap`] |
//! | `0x004bb075..0x004bb080` | GUI `Engine::render 0x00650980` | [`PassKind::Gui`] |
//! | `0x004bb080..0x004bbb1a` | HUD models over the GUI (`ZENABLE=1`, depth clear, `0x00476660`) | [`PassKind::HudModels`] |
//! | `0x004bbb1a..0x004bbb9d` | `EndScene`, `World::unlock`, destructors | – |
//!
//! # Not ported here
//!
//! - Side effects inside `render`: ambient sounds of light emitters (`0x004ac9ef..0x004ad40d`,
//!   `rand()` draws, `playSound 0x00484350`) and fire smoke particles
//!   (`0x004b1e91..0x004b212f`, `rand()`, `0x004869d0`). They consume the global `rand()`
//!   sequence, so the client controller must reproduce them in this order; they do not
//!   change what this frame draws.
//! - The creature pose builder `0x004128f0` (pose module), the map renderer `0x005fc1b0`
//!   (map module), the GUI stream (`cw-ui`). Their outputs are inputs here.

// The loops index matrices the way the original's unrolled code does.
#![allow(clippy::needless_range_loop)]
// `!(a > b)` comparisons keep the original's NaN behaviour (`comiss` + `jbe`).
#![allow(clippy::neg_cmp_op_on_partial_ord, clippy::neg_multiply)]

use crate::frame::*;

/// Fixed-point scale of world coordinates (`1 << 16` per block).
pub const FIXED_ONE: i64 = 65536;

/// `1/65536` as the original writes it (`0x006fcd94`).
const INV_FIXED: f32 = 1.525_878_9e-5;

/// Length of a day in milliseconds (`0x00702ac4`).
pub const DAY_MS: f32 = 8.64e7;

/// Clear colour of the frame (`0x004ac331`), `D3DCOLOR` ARGB.
pub const CLEAR_COLOR: u32 = 0xff00_70ff;

/// Chunk edge in blocks (`shl 5`).
pub const CHUNK_BLOCKS: i64 = 32;

/// World queries the frame needs (clouds and sky colours). Implemented by the client on top
/// of `cw-world`; kept as a trait so this crate does not depend on world generation.
pub trait WorldSampler {
    /// `climateA 0x005c4800` at block `(x, y)`.
    fn climate_a(&self, x: i32, y: i32) -> f32;
    /// `climateB 0x005c4dd0`.
    fn climate_b(&self, x: i32, y: i32) -> f32;
    /// `climateC 0x005ef040`.
    fn climate_c(&self, x: i32, y: i32) -> f32;
    /// Unnamed `0x005f0720`: a blend over the region records around `(x, y)` (it looks for
    /// neighbouring regions whose field `+0x18` is negative and weights them by distance;
    /// see the report). Used only by [`sky_colors`].
    fn climate_d(&self, x: i32, y: i32) -> f32;
    /// `World::mountainFactor 0x005effa0`.
    fn mountain_factor(&self, x: i32, y: i32) -> f32;
    /// `World::baseHeight 0x005c5e20`.
    fn base_height(&self, x: i32, y: i32) -> f32;
    /// `valueNoise2D 0x004c0ef0`.
    fn value_noise_2d(&self, x: f64, y: f64) -> f32;
}

/// A [`WorldSampler`] that returns 0 everywhere (tests, menus).
#[derive(Clone, Copy, Debug, Default)]
pub struct FlatWorld;

impl WorldSampler for FlatWorld {
    fn climate_a(&self, _: i32, _: i32) -> f32 {
        0.0
    }
    fn climate_b(&self, _: i32, _: i32) -> f32 {
        0.0
    }
    fn climate_c(&self, _: i32, _: i32) -> f32 {
        0.0
    }
    fn climate_d(&self, _: i32, _: i32) -> f32 {
        0.0
    }
    fn mountain_factor(&self, _: i32, _: i32) -> f32 {
        0.0
    }
    fn base_height(&self, _: i32, _: i32) -> f32 {
        0.0
    }
    fn value_noise_2d(&self, _: f64, _: f64) -> f32 {
        0.0
    }
}

/// Which of the three frame shapes `render` takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FrameMode {
    /// The world (the normal path).
    #[default]
    World,
    /// The loading/start screen: widget `GC+0x800898` visible (`0x004ad43a`).
    GuiOnly,
    /// The map screen: `GC+0x8006e4` set (`0x004adede`).
    MapScreen,
}

/// Camera state (`GameController` fields).
#[derive(Clone, Debug, PartialEq)]
pub struct CameraInputs {
    /// `GC+0x140`: camera (and sound listener) position, world fixed-point.
    pub position: [i64; 3],
    /// `GC+0x1d8`, `GC+0x1e0`: render offset added to world fixed-point coordinates to get
    /// render space (keeps floats small); z is always 0.
    pub render_offset: [i64; 2],
    /// `GC+0x26c`: view matrix in render space.
    pub view: D3dMatrix,
    /// `GC+0x22c`: view matrix used for the stars and the sun; `render` cancels its
    /// translation with [`CameraInputs::position`] (`0x004ae3ba..0x004ae5a1`).
    pub sky_view: D3dMatrix,
    /// `GC+0x1a4`: camera pitch in degrees as smoothed for the fog gradient.
    pub pitch: f32,
    /// `GC+0x1ac`: camera yaw in degrees (compass).
    pub yaw: f32,
    /// Local player `+0x124 & 0x400`: narrow field of view (30° instead of 50°).
    pub narrow_fov: bool,
    /// `GC+0x1d4`: fog / draw distance in blocks.
    pub fog_distance: f32,
}

/// The chunk ring window (`GC+0x2ac`, `GC+0x2b0`, `GC+0x2dc`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ChunkWindow {
    /// Chunk coordinates of the window's first chunk.
    pub origin: [i32; 2],
    /// Edge length of the window in chunks (the ring holds `size²` records).
    pub size: i32,
}

/// One `cube::ChunkBuffer` (vtable `0x006ffdb0`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ChunkBufferInput {
    /// `+0x1c`: z of the buffer's first block layer (blocks).
    pub z_base: i32,
    /// `+0x10`: vertex count.
    pub vertex_count: u32,
    /// `+0x14`: opaque triangle count.
    pub opaque_primitives: u32,
    /// `+0x18`: water triangle count.
    pub water_primitives: u32,
    /// `+8 != 0`: has an opaque index buffer.
    pub has_opaque: bool,
    /// `+0xc != 0`: has a water index buffer.
    pub has_water: bool,
}

/// A static entity that may emit light (chunk record `+0x240` list).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct EmitterInput {
    /// `+0x10`: position, world fixed-point.
    pub position: [i64; 3],
    /// `+8`: static type (0xd dims with daylight).
    pub kind: i32,
    /// `+0x40 & 1`: emits light.
    pub emits_light: bool,
    /// `+0x34..+0x3c`: light colour.
    pub color: [f32; 3],
}

/// A model drawn with the CubeShader (props, items, creature parts, projectiles).
#[derive(Clone, Debug, PartialEq)]
pub struct ModelDraw {
    /// Original call site.
    pub origin: u32,
    /// Model.
    pub model: ModelRef,
    /// World matrix, render space.
    pub world: D3dMatrix,
    /// `setMaterialColor`.
    pub material: [f32; 4],
    /// `setAlpha`.
    pub alpha: f32,
    /// Drawn with `CULLMODE=NONE` (the `0x004b7dc9..0x004b814b` group).
    pub double_sided: bool,
    /// `setShininess 0x00448fe0` for this draw (PS c4). The original sets it before a group
    /// (items and their spirit cubes: `0x004c7be0(item)`, 1 for metal) and back to 0 after,
    /// so the builder applies it to this draw only and restores 0.
    pub shininess: f32,
    /// `setWhite 0x00449090` for this draw (VS c39), or `None` to keep the white that is
    /// current (the creature's flash inside the creature pass, 0 elsewhere). Restored after
    /// the draw.
    pub set_white: Option<f32>,
    /// The model matrix mirrors (the right hand of `pose.rs`, scaled by -1 in x): the
    /// original sets `D3DRS_CULLMODE` to `D3DCULL_CCW` (3) around the draw inside
    /// 0x004128f0 and back afterwards, so the flipped winding is culled as the unmirrored
    /// parts are. The normals need nothing: the vertex shader takes the face normal through
    /// the world matrix, which turns it with the geometry.
    pub mirrored: bool,
}

impl ModelDraw {
    /// A single-sided, opaque (alpha 1), non-shiny draw that keeps the current white.
    pub fn new(origin: u32, model: ModelRef, world: D3dMatrix, material: [f32; 4]) -> ModelDraw {
        ModelDraw { origin, model, world, material, alpha: 1.0, double_sided: false, shininess: 0.0, set_white: None, mirrored: false }
    }
}

/// One record of the chunk ring (`0x268` bytes, `GC+0x2e0`).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ChunkInput {
    /// `+0x18`: chunk coordinates the record holds.
    pub coords: [i32; 2],
    /// `+0xc != 0`: the record has buffers.
    pub has_buffers: bool,
    /// `+0x20`: bounding box minimum, world fixed-point.
    pub aabb_min: [i64; 3],
    /// `+0x38`: bounding box maximum.
    pub aabb_max: [i64; 3],
    /// `+0x50`: centre used for the distance key `+0x6c`.
    pub center: [i64; 3],
    /// `+8`: buffers in list order.
    pub buffers: Vec<ChunkBufferInput>,
}

/// A dynamic point light gathered by `0x004af353..0x004b0e84`, in the original's order.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct PointLightSource {
    /// World fixed-point position.
    pub position: [i64; 3],
    /// Radius in blocks.
    pub radius: f32,
    /// Colour.
    pub color: [f32; 3],
}

/// The fields of a creature that decide its after-images (`0x004b5b62..0x004b5bb2`).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct AfterimageFields {
    /// `+0x140`.
    pub byte_140: u8,
    /// `+0x141`.
    pub byte_141: u8,
    /// `+0x6c`: time in the current action (ms).
    pub action_time: i32,
    /// `skillTotalTime 0x0043d1a0` of the creature.
    pub skill_total_time: i32,
    /// `+0x68`: current action/mode byte.
    pub mode: u8,
    /// `findBuff(0xc) != 0`.
    pub has_buff_12: bool,
    /// `findBuff(3) != 0`.
    pub has_buff_3: bool,
}

impl AfterimageFields {
    /// The original's test.
    pub fn active(&self) -> bool {
        let special = self.byte_140 == 4
            && self.byte_141 == 1
            && self.action_time < self.skill_total_time
            && matches!(self.mode, 0x11 | 0x05 | 0x14);
        special || matches!(self.mode, 0x30 | 0x5d) || self.has_buff_12 || self.has_buff_3
    }
}

/// A creature as the frame needs it. The parts come from the pose builder `0x004128f0`.
#[derive(Clone, Debug, PartialEq)]
pub struct CreatureInput {
    /// Creature id (the key of `World+4`), matched against [`RenderInputs::creature_order`].
    pub id: i64,
    /// `+0x10`: position, world fixed-point.
    pub position: [i64; 3],
    /// `+0x80[2]`: height, the culling margin.
    pub height: f32,
    /// `esp+0xbc` of `render`: the distance limit of the body's sphere test 0x004b4e1b,
    /// `GC+0x1d0 · 0.3` (0x004ad9d1..0x004ada04), shared with the creature effects.
    /// [`Default`] gives no limit.
    pub cull_distance: f32,
    /// `+0x1184`: hit flash → `white`.
    pub flash: f32,
    /// `+0x1190`: ghost amount; `> 0` defers the creature to [`PassKind::Ghosts`].
    pub ghost: f32,
    /// `+0x34`: the vector the after-images are offset along (velocity).
    pub velocity: [f32; 3],
    /// After-image rule inputs.
    pub afterimage: AfterimageFields,
    /// Posed parts (model, render-space world matrix, material), from `0x004128f0`.
    pub parts: Vec<ModelDraw>,
}

impl Default for CreatureInput {
    fn default() -> Self {
        CreatureInput {
            id: 0,
            position: [0; 3],
            height: 0.0,
            cull_distance: f32::INFINITY,
            flash: 0.0,
            ghost: 0.0,
            velocity: [0.0; 3],
            afterimage: AfterimageFields::default(),
            parts: Vec::new(),
        }
    }
}

/// A 3D model drawn in screen space by `0x00476660` (compass, portraits, item previews).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HudModel {
    /// Original call site.
    pub origin: u32,
    /// Screen position in pixels.
    pub screen: [f32; 2],
    /// Rotation in degrees about x, y, z (applied z, then y, then x).
    pub rotation: [f32; 3],
    /// Uniform scale.
    pub scale: f32,
    /// Model and its size in voxels (`+0x44..+0x4c`).
    pub model: ModelRef,
    /// Model size in voxels.
    pub model_size: [i32; 3],
    /// Depth argument.
    pub depth: f32,
    /// Material set before the call.
    pub material: [f32; 4],
}

/// Everything `render` reads, as a plain struct. Field docs name the original source.
pub struct RenderInputs<'a> {
    /// Frame shape.
    pub mode: FrameMode,
    /// `GC+0x11c`, `GC+0x120`: client width and height in pixels.
    pub screen: [i32; 2],
    /// `GC+0x8006e6`: clear to white and skip the sky and clouds.
    pub white_clear: bool,
    /// `World+0x80015c` (`GC+0x800440`): time of day in ms, 0..86.4e6.
    pub time_of_day_ms: i32,
    /// `World+0x8000bc` (`GC+0x8003a0`): millisecond clock of the light flicker.
    pub clock_ms: i32,
    /// `GC+0x8006e8`: duration of the last frame in ms.
    pub frame_ms: i32,
    /// Camera.
    pub camera: CameraInputs,
    /// Byte 3 of the block at the camera (`World::getBlockFixed`); `& 0x1f == 2` is water.
    pub camera_block_kind: u8,
    /// Local player position (`player+0x10`), gates the clouds.
    pub player_position: [i64; 3],
    /// `GC+0x1000fa4`: frustum planes of the previous frame (the emitter test runs before
    /// this frame's planes exist).
    pub previous_frustum: [[f32; 4]; 6],
    /// Chunk ring window.
    pub window: ChunkWindow,
    /// Chunk ring records, index `(y % size) * size + x % size`.
    pub chunks: Vec<ChunkInput>,
    /// Per ring record (same index as [`RenderInputs::chunks`]): the near props drawn right
    /// after the chunk's opaque buffers (`0x004bd160`), already posed. Missing records have
    /// none. `ScenePieces::chunk_props`.
    pub chunk_props: Vec<Vec<ModelDraw>>,
    /// Every point light of the frame, the static emitters of the chunk walk
    /// (`0x004ac73c..0x004acaab`) followed by the dynamic lights (`0x004af353..0x004b0e84`),
    /// **already sorted near → far** by the gatherer (`std::sort 0x004abae0` over the whole
    /// vector, before the chunk assignment `0x004b0e84` and the global 16 `0x004b12d7`).
    /// `ScenePieces::lights` through `SceneLight::source`.
    pub lights: Vec<PointLightSource>,
    /// `GC+0x80075c`: stars (direction xyz, size).
    pub stars: Vec<[f32; 4]>,
    /// `GC+0x8006e0`: sun texture.
    pub sun_texture: TextureRef,
    /// `GC+0x8006dc`: blob-shadow texture.
    pub shadow_texture: TextureRef,
    /// The cloud model drawn in every cell.
    pub cloud_model: ModelRef,
    /// Cloud model size in voxels (`+0x44`, `+0x48`).
    pub cloud_model_size: [i32; 2],
    /// Zone objects, dropped items, far props, target marker (`0x004b21ac..0x004b35d4`).
    pub objects: Vec<ModelDraw>,
    /// The posed creatures (from the pose module `0x004128f0`), in any order; drawn in
    /// [`RenderInputs::creature_order`].
    pub creatures: Vec<CreatureInput>,
    /// Creature ids in draw order, far → near (`0x004c1100` sorted by `0x004aba90`), empty
    /// while a big panel is open. `ScenePieces::creature_order`.
    pub creature_order: Vec<i64>,
    /// Non-body models of the creature phase in the original's order: airships, per creature
    /// its effects (whirls, rays, spirit cubes, markers, beams), path debug cubes, then the
    /// particles (1x1x1 cubes, double-sided, `0x004b7db6..0x004b8138`), then the projectiles
    /// (`0x004b8138..0x004b8bb9`). Drawn after the creature bodies. `ScenePieces::creature_extras`.
    pub creature_extras: Vec<ModelDraw>,
    /// Ground decals and blob shadows: each a fan of 4 textured vertices in render space.
    /// `ScenePieces::shadows`.
    pub shadows: Vec<[FixedVertex; 4]>,
    /// Ribbons: each a triangle strip of 32 untextured vertices in render space.
    /// `ScenePieces::ribbons`.
    pub ribbons: Vec<Vec<FixedVertex>>,
    /// The far list (`0x004ba462..0x004ba5d0`): props fading out between 0.18× and 0.2× of
    /// `GC+0x1d0`, statics between 0.27× and 0.3× of the draw distance, already sorted
    /// far → near and faded by the gatherer; drawn after the water. `ScenePieces::far_objects`.
    pub far_objects: Vec<ModelDraw>,
    /// `GC+0x1000e4c`: map pan in blocks, added to the player position for the map screen.
    pub map_pan: [f32; 3],
    /// `GC+0x1c4`: map zoom (mouse wheel, 0.01..10).
    pub map_zoom: f32,
    /// Minimap markers drawn after the map (`0x004758c0`).
    pub minimap_markers: Vec<ModelDraw>,
    /// The Tab quick-item wheel (`0x004bac91..0x004bb039`, in the HUD branch after the
    /// minimap and its light, before the GUI): `ZENABLE = 1`, the point lights zeroed
    /// (0x00448f10), white, then `0x004758c0` per quick item. `cw_client::ui::quick_item`.
    pub quick_wheel: Vec<GuiModel>,
    /// The GUI command stream.
    pub gui: Vec<GuiCommand>,
    /// The item models the game widgets draw during the GUI pass ([`GuiModel`]), in the
    /// widgets' order. Drawn inside [`RenderInputs::gui`] at their widget's
    /// [`GuiCommand::WidgetMark`] ([`GuiModel::anchor`]; unanchored ones after it) in every
    /// frame shape that draws the GUI. `cw_client::ui::gui_models`.
    pub gui_models: Vec<GuiModel>,
    /// The creatures the select screens' preview widgets pose and draw from their slot 1
    /// (`CharacterPreviewWidget::update` 0x00425450, `WorldPreviewWidget::update` 0x00605ae0),
    /// each with its own view, projection and light ([`GuiCreature`]). Drawn after the GUI in
    /// every frame shape that draws it. `cw_client::ui::previews`.
    pub gui_creatures: Vec<GuiCreature>,
    /// The HUD widget `GC+0x800884` (after `0x004ae27e..0x004ae370`) **and** the engine's
    /// root widget `0x00487490(GC+0x800710)` (`Engine+0xb4`) are both visible: draw the
    /// minimap and the HUD models (`0x004baa95`, `0x004bb080`). `ScenePieces::hud_visible`
    /// (the gatherer combines the two).
    pub hud_visible: bool,
    /// HUD models drawn after the GUI with the HUD light (`0x004bb259`) and no point lights.
    /// `ScenePieces::hud_models`.
    pub hud_models: Vec<HudModel>,
    /// The map screen's compass (`0x004ae21b`), drawn after the map and a depth clear with
    /// the map-compass light (`0x004ae15e`). `ScenePieces::map_compass`.
    pub map_compass: Option<HudModel>,
    /// What `cube::WorldMap::draw 0x005fc1b0` reads (tile cache, models, creatures, stored
    /// matrices). `None` leaves a bare [`Geometry::Map`] placeholder draw in the frame.
    pub map: Option<crate::map::MapScene<'a>>,
    /// World queries.
    pub world: &'a dyn WorldSampler,
}

// ---------------------------------------------------------------------------------------
// Numbers fed to the shaders
// ---------------------------------------------------------------------------------------

/// Daylight factor of `render` (`0x004ac3e1..0x004ac44a`):
/// `(1 - (2t/86.4e6 - 1)^4)^5`, 1 at noon, 0 at midnight.
pub fn daylight(time_of_day_ms: i32) -> f32 {
    let x = (time_of_day_ms as f32) * 2.0 / DAY_MS - 1.0;
    let p = (x as f64).powf(4.0) as f32;
    ((1.0 - p) as f64).powf(5.0) as f32
}

/// Daylight factor of the sky-colour function `0x0059d640`: `(1 - |2t/86.4e6 - 1|^3)^4`.
pub fn sky_daylight(time_of_day_ms: i32) -> f32 {
    let x = ((time_of_day_ms as f32) * 2.0 / DAY_MS - 1.0).abs();
    let p = (x as f64).powf(3.0) as f32;
    ((1.0 - p) as f64).powf(4.0) as f32
}

/// Sun direction (`0x004ad7a6..0x004ad822`): `a = 2π t / 86.4e6`, `(0, -sin a, -cos a)`;
/// straight up at noon. Also the sun sprite's direction.
pub fn sun_direction(time_of_day_ms: i32) -> [f32; 3] {
    let a = (((time_of_day_ms as f32) * 2.0) as f64 * std::f64::consts::PI / DAY_MS as f64) as f32;
    let c = (a as f64).cos() as f32;
    let s = (a as f64).sin() as f32;
    [0.0, -s, -c]
}

/// The colours `0x0059d640` computes for the camera position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkyColors {
    /// PS c0 `skyColor1` (third argument).
    pub sky1: [f32; 4],
    /// PS c1 `skyColor2`.
    pub sky2: [f32; 4],
    /// PS c2 `fogColor`.
    pub fog: [f32; 4],
    /// `lightFrontColor` (sun).
    pub light_front: [f32; 4],
    /// `lightBackColor`.
    pub light_back: [f32; 4],
    /// `ambientColor` of the world passes (and of the clouds).
    pub ambient: [f32; 4],
}

/// Climate values at the camera block that [`sky_colors`] needs.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct SkyClimate {
    /// `climateB` (`0x005c4dd0`).
    pub b: f32,
    /// `climateA` (`0x005c4800`).
    pub a: f32,
    /// `0x005f0720`.
    pub d: f32,
    /// `climateC` (`0x005ef040`).
    pub c: f32,
}

impl SkyClimate {
    /// Sample at block `(x, y)` in the original's call order (B, A, D only when needed, C).
    pub fn sample(world: &dyn WorldSampler, x: i32, y: i32) -> SkyClimate {
        let b = world.climate_b(x, y);
        let a = world.climate_a(x, y);
        let d = if 0.3 < b && 0.6 < a { world.climate_d(x, y) } else { 0.0 };
        let c = world.climate_c(x, y);
        SkyClimate { b, a, d, c }
    }
}

/// `0x0059d640` (5.8 KB, client-only World method): the sky gradient, fog, sun, back and
/// ambient colours for a camera position, from the time of day, three climate blends and
/// whether the camera is in water. Ported line by line; the locals keep the offsets of the
/// decompilation (`l_a8` is `local_a8`).
pub fn sky_colors(time_of_day_ms: i32, climate: SkyClimate, underwater: bool) -> SkyColors {
    let f8 = sky_daylight(time_of_day_ms);
    let mut l_a8 = 0.9f32;
    let mut l_74 = 0.9f32;
    let mut l_e0 = 0.3f32;
    let mut l_d0 = 0.3f32;
    let mut l_80 = 0.1f32;
    let mut l_98 = 0.6f32;
    let mut l_78 = 1.0f32;
    let mut l_64 = 1.0f32;
    let mut l_c0 = 1.0f32;
    let mut l_5c = 0.5f32;
    let mut l_60 = 1.0f32;
    let mut l_50 = 1.0f32;
    let mut l_a4 = 1.0f32;
    let mut l_cc = 0.01f32;
    let mut l_9c = 0.01f32;
    let mut l_90 = 0.05f32;
    let mut l_88 = 1.0f32;
    let mut l_70 = 1.0f32;
    let mut l_8c = 0.01f32;
    let mut l_30 = 0.2f32;
    let mut l_4c = 0.1f32;
    let mut l_94 = 0.8f32;
    let mut l_44 = 0.8f32;
    let mut l_38 = 1.0f32;
    let mut l_dc = 0.5f32;
    let mut l_ec = 0.0f32;
    let mut l_d4 = 0.0f32;
    let mut l_d8 = 0.5f32;
    let mut l_e8 = 1.0f32;
    let mut l_58 = 0.1f32;
    let mut l_6c = 1.0f32;
    let mut l_54 = 1.0f32;
    let mut l_a0 = 1.0f32;
    let mut l_34 = 0.9f32;
    let mut l_40 = 0.8f32;
    let mut l_48 = 1.0f32;
    let mut l_e4 = 0.5f32;
    let mut l_bc = 0.2f32;
    let mut l_20 = 0.6f32;
    let mut l_84 = 1.0f32;
    let mut l_b4 = 0.0f32;
    let mut l_b8 = 0.0f32;
    let mut l_28 = 0.1f32;
    let mut l_2c = 1.0f32;
    // local_c (`[ebp-8]`) is never written by the original: it is read uninitialised and
    // only reaches the alpha of the light colours, which no shader output uses. 0 here.
    let mut l_c = 0.0f32;
    let l_3c;
    let mut l_b0;
    let mut l_7c;
    let mut l_24;

    let mut p3 = [0.0f32; 4];
    let mut p4 = [0.0f32; 4];
    let mut p5 = [0.0f32; 4];
    let mut p6 = [0.0f32; 4];
    let mut p7 = [0.0f32; 4];
    let mut p8;

    let mut f12 = climate.b;
    let mut f10 = climate.a;
    if 0.6 < f12 && f10 < 0.4 {
        let f9 = (1.0 - f10 / 0.4) * ((f12 - 0.6) / 0.4);
        l_8c = f9 * 0.79 + 0.01;
        l_58 = f9 * 0.599_999_96 + 0.1;
        l_6c = f9 * -0.199_999_99 + 1.0;
        l_54 = f9 * 0.0 + 1.0;
    }
    if 0.3 < f12 && 0.6 < f10 {
        l_88 = ((f12 - 0.3) / 0.7) * 4.0;
        if 1.0 < l_88 {
            l_88 = 1.0;
        }
        let f9 = (1.0 - climate.d) * (((f10 - 0.6) * l_88) / 0.4);
        l_8c += f9 * (0.0 - l_8c);
        l_54 = f9 * (1.0 - l_54) + l_54;
        l_58 += f9 * (0.7 - l_58);
        l_6c += f9 * (0.5 - l_6c);
        l_98 = f9 * 0.299_999_95 + 0.6;
        l_70 = f9 * 0.0;
        l_78 = l_70 + 1.0;
        l_80 = f9 * 0.4 + 0.1;
        l_64 = l_70 + 1.0;
        l_9c = f9 * -0.01 + 0.01;
        l_90 = f9 * 0.55 + 0.05;
        l_88 = f9 * -0.100_000_024 + 1.0;
        l_70 += 1.0;
    }
    if f10 < 0.4 {
        f10 = 1.0 - f10 / 0.4;
        l_98 = (0.8 - l_98) * f10 + l_98;
        l_78 = (1.0 - l_78) * f10 + l_78;
        l_80 = (0.6 - l_80) * f10 + l_80;
        l_64 = (1.0 - l_64) * f10 + l_64;
        l_9c = (0.1 - l_9c) * f10 + l_9c;
        l_90 = (0.4 - l_90) * f10 + l_90;
        l_88 += (1.0 - l_88) * f10;
        l_70 += (1.0 - l_70) * f10;
    }
    p8 = [0.6, 0.6, 0.4, 1.0];
    if f12 < 0.2 {
        f12 /= 0.2;
        let g = 1.0 - f12;
        p8 = [g * 0.3 + f12 * 0.6, g * 0.4 + f12 * 0.6, g * 0.6 + f12 * 0.4, g + f12];
    }
    f12 = climate.c;
    if 0.0 < f12 {
        let f10 = 1.0 - f12;
        l_20 = f12 * 0.0;
        l_64 = f12 + l_64 * f10;
        l_98 = l_20 + l_98 * f10;
        l_80 = f12 + l_80 * f10;
        l_78 = l_20 + l_78 * f10;
        l_e4 = f10 * 0.5;
        l_5c = l_20 + l_e4;
        l_60 = f12 + f10;
        l_28 = f12 * 0.1;
        l_c0 = f12 * 0.8 + f10;
        l_74 = l_28 + f10 * 0.9;
        l_a4 = f12 + f10;
        l_50 = f12 * 0.8 + f10;
        l_e0 = l_28 + f10 * 0.3;
        l_d0 = l_20 + f10 * 0.3;
        l_b8 = f12 * 0.2;
        l_90 = l_b8 + l_90 * f10;
        l_9c = f12 * 0.4 + l_9c * f10;
        l_88 = l_b8 + l_88 * f10;
        l_70 = f12 + l_70 * f10;
        l_4c = l_b8 + f10 * 0.1;
        l_30 = l_b8 + f10 * 0.2;
        l_38 = f12 + f10;
        l_e8 = f12 + f10;
        l_44 = l_b8 + f10 * 0.8;
        let f9 = f10 * 0.0;
        l_dc = l_b8 + l_e4;
        l_d8 = l_b8 + l_e4;
        l_b4 = f12 * 0.3;
        l_d4 = l_b8 + f9;
        l_8c = l_b4 + l_8c * f10;
        l_58 = l_20 + l_58 * f10;
        l_6c = l_20 + l_6c * f10;
        l_34 = l_20 + f10 * 0.9;
        l_54 = f12 + l_54 * f10;
        l_40 = l_20 + f10 * 0.8;
        l_a0 = l_b4 + f10;
        l_e4 += l_b4;
        l_48 = f12 + f10;
        l_b4 += f9;
        l_20 += f10 * 0.6;
        l_bc = l_28 + f10 * 0.2;
        l_b8 += f9;
        l_84 = f12 + f10;
        l_2c = f10 + f12;
        l_28 += f10 * 0.1;
    }
    let f9 = 1.0 - f8;
    let f10 = f9 * 0.0;
    p3[1] = f8 * 0.8 + f10;
    p3[0] = f8 * 0.5 + f10;
    p3[2] = f8 + f9 * 0.2;
    p3[3] = f8 + f9;
    p4[0] = f8 * 0.0 + f10;
    p4[1] = f8 * 0.0 + f10;
    p4[2] = f8 + f10;
    p4[3] = f8 + f9;
    if f8 <= 0.75 {
        if f8 <= 0.6 {
            let f10;
            if f8 <= 0.4 {
                f10 = f8 / 0.4;
                let f9 = 1.0 - f10;
                let f11 = f9 * 0.0;
                p3[3] = f9 + f10 * l_a4;
                p3[0] = f9 * 0.05 + f10 * l_50;
                p3[1] = f11 + f10 * l_e0;
                p3[2] = f9 * 0.1 + f10 * l_d0;
                p4[2] = f9 * 0.1 + f10 * l_d8;
                p4[0] = f11 + f10 * l_dc;
                p4[3] = f9 + f10 * l_e8;
                l_a0 = f9 * l_b4;
                l_34 = f9 * l_b8;
                l_40 = f9 * l_28;
                p4[1] = f11 + f10 * l_d4;
                p5[3] = f9 * l_2c + f10 * l_84;
            } else {
                let f9 = (f8 - 0.4) / 0.2;
                f10 = 1.0 - f9;
                p3[3] = f9 * l_60 + f10 * l_a4;
                p3[0] = f9 * l_c0 + f10 * l_50;
                p3[2] = f9 * l_5c + f10 * l_d0;
                p3[1] = f9 * l_74 + f10 * l_e0;
                p4[0] = f9 * l_30 + f10 * l_dc;
                p4[2] = f9 * l_44 + f10 * l_d8;
                p4[3] = f9 * l_38 + f10 * l_e8;
                p4[1] = f9 * l_4c + f10 * l_d4;
                l_a0 *= f9;
                l_34 *= f9;
                l_40 *= f9;
                p5[3] = f9 * l_48 + f10 * l_84;
            }
            // Both inner branches continue here with their own `fVar10` (`f8 / 0.4` or
            // `1 - (f8 - 0.4) / 0.2`).
            l_40 += f10 * l_20;
            l_34 += f10 * l_bc;
            p5[0] = l_a0 + f10 * l_e4;
        } else {
            let f9 = (f8 - 0.6) / 0.15;
            let f10 = 1.0 - f9;
            p3[1] = f9 * l_98 + l_74 * f10;
            p3[2] = f9 * l_78 + l_5c * f10;
            p3[0] = f9 * l_80 + l_c0 * f10;
            p3[3] = f9 * l_64 + l_60 * f10;
            p4[1] = f9 * l_90 + l_4c * f10;
            p4[2] = f9 * l_88 + l_44 * f10;
            p4[0] = f9 * l_9c + l_30 * f10;
            p4[3] = f9 * l_70 + l_38 * f10;
            l_34 = f9 * l_58 + l_34 * f10;
            l_40 = f9 * l_6c + l_40 * f10;
            p5[0] = f9 * l_8c + l_a0 * f10;
            p5[3] = f9 * l_54 + l_48 * f10;
        }
        p5[2] = l_40;
        p5[1] = l_34;
    } else {
        p3 = [l_80, l_98, l_78, l_64];
        p4 = [l_9c, l_90, l_88, l_70];
        p5 = [l_8c, l_58, l_6c, l_54];
    }
    if underwater {
        p3 = [f8 * 0.1, f8 * 0.15, f8, 1.0];
        p4 = [f8 * 0.1, f8 * 0.15, f8, 1.0];
    }
    l_38 = 0.02;
    l_bc = 0.02;
    l_7c = 1.1f32;
    l_24 = 1.1f32;
    l_30 = 0.03;
    l_74 = 0.01;
    let mut f10 = 0.6f32;
    l_48 = 1.0;
    l_40 = 0.9;
    l_34 = 1.0;
    l_50 = 0.5;
    l_60 = 0.6;
    l_5c = 1.0;
    l_20 = 0.6;
    l_44 = 0.5;
    l_4c = 1.0;
    l_b0 = 0.0f32;
    if f12 <= 0.0 {
        l_3c = l_c;
        l_84 = l_c;
    } else {
        let f11 = 1.0 - f12;
        l_20 = f12 * 0.8;
        let f13 = f12 * 0.0;
        l_7c = f12 + f11 * 1.1;
        l_24 = l_20 + f11 * 1.1;
        l_a8 = f12 * 0.1 + f11 * 0.9;
        l_ec = f13 + f11 * 0.0;
        l_48 = l_20 + f11;
        l_34 = f13 + f11;
        let f9 = f11 * 0.6;
        l_40 = l_20 + f11 * 0.9;
        l_cc = f12 + f11 * 0.01;
        f10 = f13 + f9;
        l_38 = l_20 + f11 * 0.02;
        l_74 = f12 * 0.1 + f11 * 0.01;
        l_3c = f13 + f11 * l_c;
        l_50 = f12 + f11 * 0.5;
        l_60 = f12 * 0.5 + f9;
        l_94 = f12 * 0.5 + f11 * 0.8;
        l_5c = f13 + f11;
        l_20 += f9;
        l_44 = f12 * 0.4 + f11 * 0.5;
        l_4c = f12 * 0.4 + f11;
        l_84 = f13 + f11 * l_c;
        l_bc = f12 * 0.3 + f11 * 0.02;
        l_b0 = f12 * 0.7 + f11 * 0.0;
        l_30 = f12 * 0.3 + f11 * 0.03;
        l_c = f13 + l_c * f11;
    }
    let (f9, f10o);
    if f8 <= 0.5 {
        let f8b = f8 * 2.0;
        let f12b = 1.0 - f8b;
        p6[1] = f12b * l_38 + f8b * l_40;
        p6[2] = f12b * l_74 + f8b * f10;
        p6[3] = f12b * l_3c + f8b * l_34;
        p6[0] = f12b * l_cc + f8b * l_48;
        f9 = f12b * l_bc + f8b * l_44;
        f10o = f12b * l_30 + f8b * l_4c;
        p7[0] = f12b * l_b0 + f8b * l_20;
        p7[3] = f12b * l_c + f8b * l_84;
    } else {
        let f12b = (f8 - 0.5) * 2.0;
        let f8b = 1.0 - f12b;
        p6[0] = f12b * l_7c + f8b * l_48;
        p6[1] = f12b * l_24 + f8b * l_40;
        p6[2] = f12b * l_a8 + f8b * f10;
        p6[3] = f12b * l_ec + l_34 * f8b;
        f9 = f12b * l_60 + l_44 * f8b;
        f10o = f12b * l_94 + l_4c * f8b;
        p7[0] = f12b * l_50 + l_20 * f8b;
        p7[3] = f12b * l_5c + f8b * l_84;
    }
    p7[2] = f10o;
    p7[1] = f9;
    SkyColors { sky1: p3, sky2: p4, fog: p5, light_front: p6, light_back: p7, ambient: p8 }
}

/// Material colour of the world passes (`0x004ad684..0x004ad72f`): `(1, 1, 1, d)`, or
/// `(0.4, 0.5, 1, d)` when the camera block is water.
pub fn world_material(daylight: f32, underwater: bool) -> [f32; 4] {
    if underwater { [0.4, 0.5, 1.0, daylight] } else { [1.0, 1.0, 1.0, daylight] }
}

/// `setFogGradientTranslation 0x00448090`: `(pitch - 80) / -30`.
pub fn fog_gradient_translation(pitch: f32) -> f32 {
    (pitch - 80.0) / -30.0
}

/// `0x00427910`: `D3DXMatrixPerspectiveFovLH` layout, `aspect` as given.
pub fn perspective_lh(fov_y: f32, aspect: f32, near: f32, far: f32) -> D3dMatrix {
    let t = ((fov_y * 0.5) as f64).tan() as f32;
    let ys = 1.0 / t;
    [
        [ys / aspect, 0.0, 0.0, 0.0],
        [0.0, ys, 0.0, 0.0],
        [0.0, 0.0, far / (far - near), 1.0],
        [0.0, 0.0, -((near * far) / (far - near)), 0.0],
    ]
}

/// The world projection (`0x004ae4b3..0x004ae5d3`): 50° (30° when the player's narrow-FOV
/// flag is set), aspect `-(width / height)` (the negative aspect mirrors x for the Z-up,
/// left-handed view), near 0.1, far 2000.
pub fn projection(screen: [i32; 2], narrow_fov: bool) -> D3dMatrix {
    // The original's constants 0x3f060a92 (30°) and 0x3f5f66f4 (50°).
    let fov = if narrow_fov { f32::from_bits(0x3f06_0a92) } else { f32::from_bits(0x3f5f_66f4) };
    let aspect = -((screen[0] as f32) / (screen[1] as f32));
    perspective_lh(fov, aspect, 0.1, 2000.0)
}

/// The star/sun matrix (`0x004ae3ba..0x004ae5a1`): `GC+0x22c` with row 3 replaced by
/// `row3 + cx·row0 + cy·row1 + cz·row2`, `c` = camera position in blocks (no offset).
pub fn star_matrix(camera: &CameraInputs) -> D3dMatrix {
    let mut m = camera.sky_view;
    let c = [
        (camera.position[0] as f32) * INV_FIXED,
        (camera.position[1] as f32) * INV_FIXED,
        (camera.position[2] as f32) * INV_FIXED,
    ];
    for j in 0..4 {
        m[3][j] += c[0] * m[0][j] + c[1] * m[1][j] + c[2] * m[2][j];
    }
    m
}

/// Frustum planes (`0x004aee2e..0x004af353`) of `view · projection`, unnormalised:
/// `c3 + c0`, `c3 - c0`, `c3 + c1`, `c3 - c1`, `c3 + c2`, `c3 - c2` where `ck` is column k.
/// A point is inside when `a·x + b·y + c·z + d >= 0` for all six.
pub fn frustum_planes(view: &D3dMatrix, projection: &D3dMatrix) -> [[f32; 4]; 6] {
    let m = mat_mul(view, projection);
    let col = |j: usize| [m[0][j], m[1][j], m[2][j], m[3][j]];
    let (c0, c1, c2, c3) = (col(0), col(1), col(2), col(3));
    let add = |a: [f32; 4], b: [f32; 4]| [a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3]];
    let sub = |a: [f32; 4], b: [f32; 4]| [a[0] - b[0], a[1] - b[1], a[2] - b[2], a[3] - b[3]];
    [add(c3, c0), sub(c3, c0), add(c3, c1), sub(c3, c1), add(c3, c2), sub(c3, c2)]
}

/// `vec3i64::dot 0x0043ac20`: fixed-point dot product, each product divided by 65536 with
/// truncation.
fn dot_fixed(a: [i64; 3], b: [i64; 3]) -> i64 {
    (a[0].wrapping_mul(b[0])) / FIXED_ONE + (a[1].wrapping_mul(b[1])) / FIXED_ONE
        + (a[2].wrapping_mul(b[2])) / FIXED_ONE
}

/// `__ftol2`: truncation toward zero.
fn ftol(v: f32) -> i64 {
    v as i64
}

/// `0x0047f760`: sphere visibility. Visible when the squared 3D distance from the camera
/// (fixed-point, `dot`) is at most `max_distance² · 65536` and the point, pushed out by
/// `margin`, is on the inner side of all six planes. Plane coordinates are render space
/// for x/y and raw blocks for z (the original adds the render offset to x and y only).
pub fn sphere_visible(
    planes: &[[f32; 4]; 6],
    camera: &CameraInputs,
    position: [i64; 3],
    margin: f32,
    max_distance: f32,
) -> bool {
    let d = [
        camera.position[0] - position[0],
        camera.position[1] - position[1],
        camera.position[2] - position[2],
    ];
    if dot_fixed(d, d) > ftol(max_distance * max_distance * 65536.0) {
        return false;
    }
    let x = ((position[0] + camera.render_offset[0]) as f32) * INV_FIXED;
    let y = ((position[1] + camera.render_offset[1]) as f32) * INV_FIXED;
    // 0x0047f8c1: z is scaled once, before the plane loop (`xmm2 = f32(z) · 1/65536`).
    let z = (position[2] as f32) * INV_FIXED;
    // 0x0047f8d0..0x0047f8fe: `a·x + b·y + c·z + d + margin`, `comiss sum, 0; ja reject`: only
    // `0 > sum` rejects, so a NaN sum passes.
    planes.iter().all(|p| !(0.0 > p[0] * x + p[1] * y + p[2] * z + p[3] + margin))
}

/// `0x0047f3c0`: box visibility. The horizontal distance from the camera to the box centre
/// `(min + max) / 2` must be at most `max_distance`, and the box's positive vertex (per
/// plane: max where the normal component is positive, else min) must be inside all six
/// planes.
pub fn box_visible(
    planes: &[[f32; 4]; 6],
    camera: &CameraInputs,
    min: [i64; 3],
    max: [i64; 3],
    max_distance: f32,
) -> bool {
    let cx = (min[0] + max[0]) * 32768 / FIXED_ONE;
    let cy = (min[1] + max[1]) * 32768 / FIXED_ONE;
    let dx = camera.position[0] - cx;
    let dy = camera.position[1] - cy;
    let d2 = dx.wrapping_mul(dx) / FIXED_ONE + dy.wrapping_mul(dy) / FIXED_ONE;
    if d2 > ftol(max_distance * max_distance * 65536.0) {
        return false;
    }
    planes.iter().all(|p| {
        let x = if p[0] <= 0.0 { min[0] } else { max[0] } + camera.render_offset[0];
        let y = if p[1] <= 0.0 { min[1] } else { max[1] } + camera.render_offset[1];
        let z = if p[2] <= 0.0 { min[2] } else { max[2] };
        let v = p[1] * (y as f32) * INV_FIXED + p[0] * (x as f32) * INV_FIXED + p[2] * (z as f32) * INV_FIXED + p[3];
        v >= 0.0
    })
}

/// Render-space position of a world fixed-point point (`0x0042c800` add offset, then
/// `vec3f::fromFixed 0x0042c4a0`).
pub fn render_position(camera: &CameraInputs, p: [i64; 3]) -> [f32; 3] {
    [
        ((p[0] + camera.render_offset[0]) as f32) * INV_FIXED,
        ((p[1] + camera.render_offset[1]) as f32) * INV_FIXED,
        (p[2] as f32) * INV_FIXED,
    ]
}

/// Draw distance of the chunk and light tests: `GC+0x1d4 + 22.4` (`0x004ac4c7`).
pub fn draw_distance(camera: &CameraInputs) -> f32 {
    camera.fog_distance + 22.4
}

// ---------------------------------------------------------------------------------------
// Chunks and point lights
// ---------------------------------------------------------------------------------------

fn ring_index(window: &ChunkWindow, x: i32, y: i32) -> usize {
    let n = window.size;
    ((y % n) * n + x % n) as usize
}

/// The chunk window walk of `0x004ac4c7..0x004ad434`, visibility part: a record whose
/// coordinates match and that has buffers is visible when `0x0047f3c0` accepts its box
/// with distance `draw_distance + 32`; its key `+0x6c` is the squared 3D distance from its
/// centre to the camera in blocks². The result is sorted by that key, near to far with
/// the MSVC 2012 `std::sort 0x004aba70` (introsort, not stable: [`cw_math::sort::msvc_sort`]
/// reproduces its arrangement of equal keys). A white-cleared frame has none (`0x004b1af4`
/// clears the list).
pub fn collect_visible_chunks(inputs: &RenderInputs, planes: &[[f32; 4]; 6]) -> Vec<(usize, f32)> {
    let w = &inputs.window;
    let mut out = Vec::new();
    if w.size <= 0 || inputs.white_clear {
        return out;
    }
    let dist = draw_distance(&inputs.camera) + 32.0;
    for x in w.origin[0]..w.origin[0] + w.size {
        for y in w.origin[1]..w.origin[1] + w.size {
            if x < 0 || y < 0 || x >= 0x80000 || y >= 0x80000 {
                continue;
            }
            let idx = ring_index(w, x, y);
            let Some(c) = inputs.chunks.get(idx) else { continue };
            if !c.has_buffers || c.coords != [x, y] {
                continue;
            }
            if !box_visible(planes, &inputs.camera, c.aabb_min, c.aabb_max, dist) {
                continue;
            }
            let d = [
                c.center[0] - inputs.camera.position[0],
                c.center[1] - inputs.camera.position[1],
                c.center[2] - inputs.camera.position[2],
            ];
            let key = (dot_fixed(d, d) as f32) * INV_FIXED;
            out.push((idx, key));
        }
    }
    cw_math::sort::msvc_sort(&mut out, |a, b| a.1 < b.1);
    out
}

/// A gathered point light: render-space light plus its world position (for the chunk
/// assignment, which works in world blocks).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GatheredLight {
    /// World fixed-point position.
    pub world: [i64; 3],
    /// The light as uploaded.
    pub light: PointLight,
}

/// `0x004b0e84..0x004b12d7`: every chunk of the window starts with 16 zero slots; each
/// light, in order, is appended to every window chunk its square `[p - r, p + r]` touches
/// (chunk index = block / 32 truncated toward zero), while the chunk has fewer than 16.
/// Returns one [`LightSet`] per ring record.
pub fn assign_lights_to_chunks(window: &ChunkWindow, ring_len: usize, lights: &[GatheredLight]) -> Vec<LightSet> {
    let mut sets = vec![LightSet::default(); ring_len];
    let n = window.size;
    if n <= 0 {
        return sets;
    }
    for g in lights {
        let r = ftol(g.light.radius * 65536.0);
        let to_chunk = |v: i64| ((v / FIXED_ONE) / CHUNK_BLOCKS) as i32;
        let (x0, x1) = (to_chunk(g.world[0] - r), to_chunk(g.world[0] + r));
        let (y0, y1) = (to_chunk(g.world[1] - r), to_chunk(g.world[1] + r));
        for x in x0..=x1 {
            for y in y0..=y1 {
                let inside = window.origin[0] <= x
                    && x < window.origin[0] + n
                    && window.origin[1] <= y
                    && y < window.origin[1] + n
                    && x >= 0
                    && y >= 0
                    && x < 0x80000
                    && y < 0x80000;
                if !inside {
                    continue;
                }
                let idx = ring_index(window, x, y);
                if let Some(set) = sets.get_mut(idx)
                    && set.lights.len() < LIGHT_SET_CAPACITY
                {
                    set.lights.push(g.light);
                }
            }
        }
    }
    sets
}

// ---------------------------------------------------------------------------------------
// Matrices of the model helpers
// ---------------------------------------------------------------------------------------

/// `0x00424a60`/`0x00424990`: `M = T(t) · M` (translate in object space).
pub fn pre_translate(m: &mut D3dMatrix, t: [f32; 3]) {
    for j in 0..4 {
        m[3][j] += m[1][j] * t[1] + m[0][j] * t[0] + m[2][j] * t[2];
    }
}

/// `0x00424730`: scale rows 0..2 (each skipped when its factor is exactly 1).
pub fn pre_scale(m: &mut D3dMatrix, s: [f32; 3]) {
    for (i, &k) in s.iter().enumerate() {
        if k != 1.0 {
            for j in 0..4 {
                m[i][j] *= k;
            }
        }
    }
}

/// `mat4RotateZ 0x00424610`: `M = Rz(deg) · M`.
pub fn pre_rotate_z(m: &mut D3dMatrix, degrees: f32) {
    let r = (degrees * 0.017_453_292) as f64;
    let (c, s) = (r.cos() as f32, r.sin() as f32);
    for j in 0..4 {
        let a = m[0][j];
        m[0][j] = m[1][j] * s + a * c;
        m[1][j] = m[1][j] * c - a * s;
    }
}

fn translation(t: [f32; 3]) -> D3dMatrix {
    let mut m = IDENTITY;
    m[3][0] = t[0];
    m[3][1] = t[1];
    m[3][2] = t[2];
    m
}

/// World and projection of a HUD model (`0x00476660`): the model is centred on its size,
/// scaled, rotated (z, then y, then x, degrees), placed at view depth 1; the projection is
/// `PerspectiveLH(45°, -(w/h), 0.1, ~1000) · T(ndc_x, ndc_y, depth)` (the offset added in
/// clip space, so the model centre lands on the pixel) written with the literal constants
/// 1.0001 and -0.10001; the view is identity.
pub fn hud_model_matrices(screen: [i32; 2], h: &HudModel) -> (D3dMatrix, D3dMatrix) {
    let rad = |d: f32| (d * 0.017_453_292) as f64;
    let mut m = IDENTITY;
    // M = Rx · M, then Ry, then Rz (each pre-multiplied), matching the three blocks.
    let (c, s) = (rad(h.rotation[0]).cos() as f32, rad(h.rotation[0]).sin() as f32);
    for j in 0..4 {
        let (r1, r2) = (m[1][j], m[2][j]);
        m[1][j] = r2 * s + r1 * c;
        m[2][j] = r2 * c - r1 * s;
    }
    let (c, s) = (rad(h.rotation[1]).cos() as f32, rad(h.rotation[1]).sin() as f32);
    for j in 0..4 {
        let (r0, r2) = (m[0][j], m[2][j]);
        m[2][j] = r2 * c + r0 * s;
        m[0][j] = r0 * c - r2 * s;
    }
    let (c, s) = (rad(h.rotation[2]).cos() as f32, rad(h.rotation[2]).sin() as f32);
    for j in 0..4 {
        let (r0, r1) = (m[0][j], m[1][j]);
        m[0][j] = r1 * s + r0 * c;
        m[1][j] = r1 * c - r0 * s;
    }
    if h.scale != 1.0 {
        for row in m.iter_mut().take(3) {
            for v in row.iter_mut() {
                *v *= h.scale;
            }
        }
    }
    let half = [
        h.model_size[0] as f32 * -0.5,
        h.model_size[1] as f32 * -0.5,
        h.model_size[2] as f32 * -0.5,
    ];
    for j in 0..4 {
        m[3][j] += m[1][j] * half[1] + m[0][j] * half[0] + m[2][j] * half[2];
    }
    // The translation (0, 0, 1) the original folds in before rotating.
    m[3][2] += 1.0;
    (m, screen_projection(screen, h.screen, h.depth))
}

/// The projection of the screen-space model draws `0x00476660` / `0x004758c0`:
/// `P = 0x00427910(45°, -(w/h), 0.1, 1000)`, `T` an identity whose row 3 gets `row0·ndc_x +
/// row1·ndc_y + row2·depth` (0x0047645b..0x00476573), and the product `0x00424f30(this = T,
/// out, P)`, which computes `out[r][c] = Σ P[r][k]·T[k][c]`: `P · T`, the offset added in
/// clip space (`ndc += (ndc_x, ndc_y, depth)`), where `ndc = ((p - size/2) / size) · (2, -2)`.
/// `0x00476660` inlines the same product (0x00476fe6..0x004771cf).
fn screen_projection(screen: [i32; 2], pos: [f32; 2], depth: f32) -> D3dMatrix {
    let (w, hgt) = (screen[0] as f32, screen[1] as f32);
    let ndc_x = ((pos[0] - w * 0.5) / w) * 2.0;
    let ndc_y = ((pos[1] - hgt * 0.5) / hgt) * -2.0;
    let f = 1.0 / ((0.392_699_09f64).tan() as f32);
    let fx = f / -(w / hgt);
    let persp = [
        [fx, 0.0, 0.0, 0.0],
        [0.0, f, 0.0, 0.0],
        [0.0, 0.0, 1.0001, 1.0],
        [0.0, 0.0, -0.10001, 0.0],
    ];
    mat_mul(&persp, &translation([ndc_x, ndc_y, depth]))
}

/// An item model drawn inside the GUI by `GameController 0x004758c0(x, y, float3* rotation,
/// scale, Item*, depth)` from a game widget's slot-1 body: the inventory cells
/// (`InventoryWidget` 0x004c2050), the equipment boxes (`SpriteWidget` 0x0051c3d0), the item
/// preview (`PreviewWidget` 0x004d50a0), the identification / adaption icons (0x0044ea30,
/// 0x0040f8f0), the recipe ingredients (0x0042f910) and the customization list (0x00588500).
///
/// What `0x004758c0` does, in order: `ZENABLE = 1`, `ZFUNC = LESS` (render states 7 and 0x17,
/// the ZFUNC site of `analysis/shaders/README.md`); the world matrix of
/// [`gui_model_matrices`]; the projection of the HUD models with the view identity; bind VS00
/// + PS01 (0x00447d10), alpha 1 (0x00447fb0), shininess `0x004c7be0(item)` (0x00448fe0);
/// `cube::Model::draw` 0x004e6df0; then the spirit cubes `0x00471b60(item, world, view,
/// projection, (1, 1, 1, 1), 0)`. The material colour and the (zeroed) point lights are the
/// caller's (`0x00448280`, `0x00448f10`), the directional light whatever was set last.
///
/// The model is `ModelCache::modelForItem 0x004ec400(item)`, except for an item of type 0,
/// which draws model `0x95c` of the model vector (`GC+0x304 + 0x2570`) when the vector is that
/// long; no model, no draw.
#[derive(Clone, Debug, PartialEq)]
pub struct GuiModel {
    /// Original call site of `0x004758c0`.
    pub origin: u32,
    /// `(x, y)`: screen position of the model centre, pixels.
    pub screen: [f32; 2],
    /// `*rotation`: Euler angles in degrees (applied x, then y, then z), `GC+0x800a1c` for
    /// every caller.
    pub rotation: [f32; 3],
    /// The scale argument (before the fit to the largest dimension).
    pub scale: f32,
    /// The item is a ring (type 9): two more quarter turns (`0x00475d8e..`).
    pub ring_turn: bool,
    /// The model and its size in voxels (`+0x44..+0x4c`).
    pub model: ModelRef,
    /// Model size in voxels.
    pub model_size: [i32; 3],
    /// The depth argument (0, or -0.05 for the item preview).
    pub depth: f32,
    /// Material colour set before the call (`setMaterialColor 0x00448280`).
    pub material: [f32; 4],
    /// `0x004c7be0(item)`: 1 for the metal materials, else 0.
    pub shininess: f32,
    /// The spirit cubes of `0x00471b60`: per cube its offset in model voxels (`item+0x14 +
    /// 8i`, signed bytes) and its colour (`0x004c7250`).
    pub spirits: Vec<([f32; 3], [f32; 4])>,
    /// The unit cube the spirit cubes are drawn with.
    pub spirit_model: ModelRef,
    /// The widget whose slot 1 draws it: the model is drawn at that
    /// [`GuiCommand::WidgetMark`] of the GUI stream (dropped when the stream has no such mark,
    /// the widget's node was not drawn). `None`: after the whole GUI.
    pub anchor: Option<GuiAnchor>,
}

/// A posed creature (or the world preview model, `Model::draw` 0x004e6df0 at 0x00607033 with
/// the same state and its own light) drawn inside the GUI by a select-screen preview widget:
/// the character cards (`CharacterPreviewWidget::update` `Cube.exe 0x00425450`, call at 0x004269e1) and the
/// selected character walking on its world's card (`WorldPreviewWidget::update` 0x00605ae0,
/// call at 0x00607e31). What the widget does around `0x004128f0`, in order: `ZENABLE = 1`
/// (render state 7), the 16 point lights zeroed (0x00448f10), bind VS00 + PS01 (0x00447d10),
/// alpha 1 (0x00447fb0), shininess 0 (0x00448fe0), material (1, 1, 1, 1) (0x00448280), camera
/// position (0, 0, 0) (0x00448010), `setLight` 0x00448170 with front (1, 1, 1, 1), back
/// (0.2, 0.3, 0.4, 1), [`GuiCreature::light_direction`] and ambient (0.4, 0.4, 0.4, 1), fog
/// distance 1e9 (0x00448100(0x4e6e6b28)); then `0x004128f0(models, view, projection, ...)`,
/// which sets the transforms and draws the parts; then `CULLMODE = NONE` (render state 0x16
/// = 1) and `ZENABLE = 0`. No depth clear: the GUI pass's beginFrame cleared depth.
#[derive(Clone, Debug, PartialEq)]
pub struct GuiCreature {
    /// Call site of `0x004128f0` (0x004269e1 for a character card, 0x00607e31 for the walker
    /// on a world card).
    pub origin: u32,
    /// The view matrix the widget builds (camera 17 units out, pitch -95 degrees, yaw 45
    /// degrees, the creature's z taken out).
    pub view: D3dMatrix,
    /// The perspective of the widget, shifted so the creature lands on its card.
    pub projection: D3dMatrix,
    /// The light direction before `setLight` normalises it.
    pub light_direction: [f32; 3],
    /// The posed parts (`0x004128f0` with the creature's x/y taken out through its origin
    /// arguments), in the pose's draw order.
    pub parts: Vec<ModelDraw>,
}

fn rows_rotate(m: &mut D3dMatrix, a: usize, b: usize, c: f32, s: f32) {
    // `ra' = ra·c + rb·s`, `rb' = rb·c - ra·s` (the unrolled blocks of 0x004758c0).
    for j in 0..4 {
        let (ra, rb) = (m[a][j], m[b][j]);
        m[a][j] = ra * c + rb * s;
        m[b][j] = rb * c - ra * s;
    }
}

/// The world matrix of `0x004758c0` (the projection is the HUD models' one):
///
/// 1. identity, then `T(0, 0, 1)` (`0x00475908`);
/// 2. the rotations by `rotation` x, y, z in degrees (`· 0.017453292`, `cos`/`sin` in double),
///    each pre-multiplied (rows 1/2, then 2/0, then 0/1);
/// 3. a ring (type 9): a further turn of `+π/2` on rows 1/2 and `-π/2` on rows 0/1 (the float
///    `π/2` widened, `1.5707963705062866`);
/// 4. rows 0..2 scaled by `scale` when it is not 1, then by `1 / (float)max(size)` when that
///    is not 1;
/// 5. the translation `(-sx/2, -sy/2, -sz/2)` in model space.
pub fn gui_model_matrices(screen: [i32; 2], g: &GuiModel) -> (D3dMatrix, D3dMatrix) {
    let rad = |d: f32| (d * 0.017_453_292) as f64;
    let mut m = IDENTITY;
    for j in 0..4 {
        m[3][j] += m[2][j];
    }
    let (c, s) = (rad(g.rotation[0]).cos() as f32, rad(g.rotation[0]).sin() as f32);
    rows_rotate(&mut m, 1, 2, c, s);
    let (c, s) = (rad(g.rotation[1]).cos() as f32, rad(g.rotation[1]).sin() as f32);
    rows_rotate(&mut m, 2, 0, c, s);
    let (c, s) = (rad(g.rotation[2]).cos() as f32, rad(g.rotation[2]).sin() as f32);
    rows_rotate(&mut m, 0, 1, c, s);
    if g.ring_turn {
        let q = 1.570_796_370_506_286_6f64;
        rows_rotate(&mut m, 1, 2, q.cos() as f32, q.sin() as f32);
        rows_rotate(&mut m, 0, 1, (-q).cos() as f32, (-q).sin() as f32);
    }
    if g.scale != 1.0 {
        for row in m.iter_mut().take(3) {
            for v in row.iter_mut() {
                *v *= g.scale;
            }
        }
    }
    let big = g.model_size[0].max(g.model_size[1]).max(g.model_size[2]);
    let f = 1.0 / (big as f32);
    if f != 1.0 {
        for row in m.iter_mut().take(3) {
            for v in row.iter_mut() {
                *v *= f;
            }
        }
    }
    let h = [g.model_size[0] as f32 * -0.5, g.model_size[1] as f32 * -0.5, g.model_size[2] as f32 * -0.5];
    for j in 0..4 {
        m[3][j] += m[0][j] * h[0] + m[1][j] * h[1] + m[2][j] * h[2];
    }
    (m, screen_projection(screen, g.screen, g.depth))
}

// ---------------------------------------------------------------------------------------
// Frame builder
// ---------------------------------------------------------------------------------------

/// Tracks the device state as the original sets it and records draws with snapshots.
struct Builder {
    out: FrameCommands,
    state: PipelineState,
    uniforms: CubeUniforms,
    uniforms_dirty: bool,
    draw: DrawUniforms,
    fixed_transforms: [D3dMatrix; 3],
}

impl Builder {
    fn new(uniforms: CubeUniforms) -> Builder {
        Builder {
            out: FrameCommands {
                passes: Vec::new(),
                uniforms: Vec::new(),
                light_sets: vec![LightSet::default()],
                frustum_planes: None,
                projection: None,
            },
            // The state the previous frame's GUI (`beginFrame 0x00688b60`) and HUD models
            // (`0x00476660`: ZENABLE 1, ZFUNC LESS) leave; `render` never resets blending.
            state: PipelineState {
                program: Program::GUI,
                depth: DepthState { test: false, write: true, func: CompareFunc::LessEqual },
                blend: BlendState::ALPHA,
                cull: Cull::None,
                color_write: 0xf,
                sampler0: SamplerState::DEFAULT,
            },
            uniforms,
            uniforms_dirty: true,
            draw: DrawUniforms {
                world: IDENTITY,
                material: [1.0; 4],
                alpha: 1.0,
                white: 0.0,
                shininess: 0.0,
                light_set: 0,
            },
            fixed_transforms: [IDENTITY; 3],
        }
    }

    fn begin(&mut self, kind: PassKind, range: (u32, u32), clear: Option<Clear>) {
        self.out.passes.push(Pass {
            kind,
            range,
            target: RenderTarget::Backbuffer,
            clear,
            state: self.state,
            draws: Vec::new(),
        });
    }

    /// Change state; if the current pass has no draws yet its start state follows.
    fn set(&mut self, f: impl FnOnce(&mut PipelineState)) {
        f(&mut self.state);
        if let Some(p) = self.out.passes.last_mut()
            && p.draws.is_empty()
        {
            p.state = self.state;
        }
    }

    fn uni(&mut self, f: impl FnOnce(&mut CubeUniforms)) {
        f(&mut self.uniforms);
        self.uniforms_dirty = true;
    }

    fn light_set(&mut self, set: LightSet) -> usize {
        self.out.light_sets.push(set);
        self.out.light_sets.len() - 1
    }

    fn emit(&mut self, origin: u32, geometry: Geometry) {
        if self.uniforms_dirty {
            self.out.uniforms.push(self.uniforms.clone());
            self.uniforms_dirty = false;
        }
        let draw = Draw {
            origin,
            state: self.state,
            uniforms: self.out.uniforms.len() - 1,
            draw: self.draw.clone(),
            geometry,
        };
        self.out.passes.last_mut().expect("draw outside a pass").draws.push(draw);
    }

    fn model(&mut self, m: &ModelDraw) {
        self.draw.world = m.world;
        self.draw.material = m.material;
        self.draw.alpha = m.alpha;
        // setShininess / setWhite around the draw (0x00471b60, items 0x004b2c1b): restored
        // afterwards, shininess to the 0 the original writes back.
        let prev_white = self.draw.white;
        self.draw.shininess = m.shininess;
        if let Some(w) = m.set_white {
            self.draw.white = w;
        }
        if m.double_sided {
            let prev = self.state.cull;
            self.set(|s| s.cull = Cull::None);
            self.emit(m.origin, Geometry::Model { model: m.model });
            self.set(|s| s.cull = prev);
        } else if m.mirrored {
            // The mirrored hand: `SetRenderState(CULLMODE, CCW)` 0x00423588, the draw, then
            // `SetRenderState(CULLMODE, CW)` 0x004235bf (set, not restored).
            self.set(|s| s.cull = Cull::Ccw);
            self.emit(m.origin, Geometry::Model { model: m.model });
            self.set(|s| s.cull = Cull::Cw);
        } else {
            self.emit(m.origin, Geometry::Model { model: m.model });
        }
        self.draw.shininess = 0.0;
        self.draw.white = prev_white;
    }

    /// `setLight 0x00448170(front, back, direction, ambient)`: the setter normalises the
    /// direction.
    fn set_light(&mut self, front: [f32; 4], back: [f32; 4], direction: [f32; 3], ambient: [f32; 4]) {
        self.uni(|u| {
            u.light_front = front;
            u.light_back = back;
            u.light_direction = normalize(direction);
            u.ambient = ambient;
        });
    }

    /// One `cube::WorldMap::draw 0x005fc1b0` call at the current point of the frame: the
    /// map module's draws spliced into the current pass and the state it leaves behind
    /// (`map.rs`, "Hook for `passes.rs`"); a bare [`Geometry::Map`] when no map scene is
    /// given.
    fn map(&mut self, origin: u32, view: MapView, scene: Option<&crate::map::MapScene>) {
        let Some(scene) = scene else {
            self.emit(origin, Geometry::Map(view));
            return;
        };
        if self.uniforms_dirty {
            self.out.uniforms.push(self.uniforms.clone());
            self.uniforms_dirty = false;
        }
        let ctx = crate::map::MapContext { state: self.state, uniforms: self.uniforms.clone(), draw: self.draw.clone() };
        let draws = crate::map::map_draws(&view, scene, &ctx, &mut self.out.uniforms);
        let (state, uniforms) = crate::map::map_exit(&ctx, &draws);
        self.out.passes.last_mut().expect("map outside a pass").draws.extend(draws);
        self.set(|s| *s = state);
        self.uniforms = uniforms;
        self.uniforms_dirty = true;
        // 0x005fce1f: the map zeroed the point lights.
        self.draw.light_set = 0;
    }

    fn hud_model(&mut self, screen: [i32; 2], h: &HudModel) {
        // 0x00476660: ZENABLE=1, ZFUNC=LESS, bind VS00+PS01, identity view, alpha 1,
        // shininess 0.
        let (world, proj) = hud_model_matrices(screen, h);
        self.set(|s| {
            s.depth.test = true;
            s.depth.func = CompareFunc::Less;
            s.program = Program::WORLD;
        });
        self.uni(|u| {
            u.view = IDENTITY;
            u.projection = proj;
        });
        self.draw.world = world;
        self.draw.material = h.material;
        self.draw.alpha = 1.0;
        self.draw.shininess = 0.0;
        self.emit(h.origin, Geometry::Model { model: h.model });
    }

    /// `0x004758c0` for each GUI model of one widget mark (or after the GUI): a pass without clear on the
    /// back buffer (the GUI pass's `beginFrame` cleared depth; the GUI itself does not
    /// touch it), the point lights zeroed (0x00448f10 by every caller), the light left as
    /// it was.
    fn gui_models(&mut self, screen: [i32; 2], models: &[GuiModel]) {
        if models.is_empty() {
            return;
        }
        self.begin(PassKind::GuiModels, (0x0065_0980, 0x0065_0980), None);
        self.draw.light_set = 0;
        for g in models {
            self.gui_model(screen, g);
        }
    }

    fn gui_model(&mut self, screen: [i32; 2], g: &GuiModel) {
        let big = g.model_size[0].max(g.model_size[1]).max(g.model_size[2]);
        if big <= 0 {
            return;
        }
        // 0x004758c0: ZENABLE=1, ZFUNC=LESS, bind VS00+PS01, identity view.
        let (world, proj) = gui_model_matrices(screen, g);
        self.set(|s| {
            s.depth.test = true;
            s.depth.write = true;
            s.depth.func = CompareFunc::Less;
            s.program = Program::WORLD;
        });
        self.uni(|u| {
            u.view = IDENTITY;
            u.projection = proj;
        });
        self.draw.world = world;
        self.draw.material = g.material;
        self.draw.alpha = 1.0;
        self.draw.shininess = g.shininess;
        self.emit(g.origin, Geometry::Model { model: g.model });
        // 0x00471b60: one unit cube per spirit at `world · T(offset)`, in its colour.
        for (off, color) in &g.spirits {
            let mut m = world;
            pre_translate(&mut m, *off);
            self.draw.world = m;
            self.draw.material = *color;
            self.emit(0x0047_1cfc, Geometry::Model { model: g.spirit_model });
        }
        self.draw.shininess = 0.0;
    }

    /// The select-screen preview creatures ([`GuiCreature`]), after the GUI: a pass without
    /// clear, then per creature the state and constants of 0x00425450 / 0x00605ae0 and its
    /// parts.
    fn gui_creatures(&mut self, list: &[GuiCreature]) {
        if list.is_empty() {
            return;
        }
        self.begin(PassKind::GuiModels, (0x0065_0980, 0x0065_0980), None);
        for c in list {
            // ZENABLE = 1, bind VS00 + PS01.
            self.set(|s| {
                s.depth.test = true;
                s.depth.write = true;
                s.program = Program::WORLD;
            });
            // 0x00448f10 (zeroed point lights), alpha 1, shininess 0, white 0 (the pose's
            // colour argument is (1, 1, 1, 1)).
            self.draw.light_set = 0;
            self.draw.alpha = 1.0;
            self.draw.shininess = 0.0;
            self.draw.white = 0.0;
            self.uni(|u| {
                u.view = c.view;
                u.projection = c.projection;
                u.camera_position = [0.0; 3];
                // setFogScale(1e9) 0x00448100.
                u.fog_scale = 1.0 / 1.0e9;
            });
            self.set_light([1.0; 4], [0.2, 0.3, 0.4, 1.0], c.light_direction, [0.4, 0.4, 0.4, 1.0]);
            for p in &c.parts {
                self.model(p);
            }
            // CULLMODE = NONE, ZENABLE = 0.
            self.set(|s| {
                s.cull = Cull::None;
                s.depth.test = false;
            });
        }
    }

    /// The GUI state of beginFrame 0x00688b60: CULL NONE, LIGHTING 0, ZENABLE 0, STENCIL 0,
    /// blend on, separate alpha ONE/INVSRCALPHA, s1 BORDER.
    fn gui_state(&mut self) {
        self.set(|s| {
            s.program = Program::GUI;
            s.cull = Cull::None;
            s.depth.test = false;
            s.blend = BlendState::ALPHA;
        });
    }

    /// `Engine::render` 0x00650980 (called at 0x004bb07b) with the widgets' `0x004758c0` item
    /// models drawn where the original draws them: inside the traversal, at the slot-1 point
    /// of their widget (`Node::render` 0x00632910 calls the widget's slot 1 after the node's
    /// shape and before its children), marked in the stream by [`GuiCommand::WidgetMark`].
    /// Everything the GUI draws after a model (its widget's texts drawn after it, such as the
    /// inventory's stack counts, `InventoryWidget::update` 0x004c2050: models 0x004c2577, then
    /// `drawText` 0x00639b30 from 0x004c2862; later siblings such as the quick-item button's
    /// `count` after its `background` sprite; later panels) covers it, since the GUI draws
    /// without the depth test (the widgets reset ZENABLE, render state 7, to 0 after their
    /// models, e.g. 0x004c25bc / 0x0051c74a).
    ///
    /// The stream is split into one [`Geometry::Gui`] per run between marks: the first pass
    /// clears depth and stencil (beginFrame), each model run is a [`PassKind::GuiModels`]
    /// pass, each continuation a [`PassKind::Gui`] pass without clear. A mark inside a render
    /// surface (a clipping or blurred ancestor) does not split the stream (the executor keeps
    /// its surface stack per `Geometry::Gui`); its models wait until the stream is back on
    /// the back buffer and past the level's composite (the next unmasked draw or mark), where
    /// the original draws them into the level surface (see the report). Models without an
    /// anchor are drawn after the GUI.
    fn gui_with_models(&mut self, screen: [i32; 2], gui: &[GuiCommand], models: &[GuiModel]) {
        self.gui_state();
        self.begin(
            PassKind::Gui,
            (0x004b_b075, 0x004b_b080),
            Some(Clear { color_argb: None, depth: Some(1.0), stencil: Some(0) }),
        );
        let mut run: Vec<GuiCommand> = Vec::new();
        let mut pending: Vec<GuiAnchor> = Vec::new();
        let mut depth = 0u32;
        for c in gui {
            match c {
                GuiCommand::WidgetMark(a) => {
                    pending.push(*a);
                    if depth == 0 {
                        self.gui_run(screen, &mut run, &mut pending, models, false);
                    }
                    continue;
                }
                GuiCommand::Draw(g) if depth == 0 && g.mask_surface.is_none() && !pending.is_empty() => {
                    self.gui_run(screen, &mut run, &mut pending, models, false);
                }
                GuiCommand::BeginSurface { .. } => depth += 1,
                GuiCommand::EndSurface => depth = depth.saturating_sub(1),
                _ => {}
            }
            run.push(c.clone());
        }
        self.gui_run(screen, &mut run, &mut pending, models, true);
        let rest: Vec<GuiModel> = models.iter().filter(|g| g.anchor.is_none()).cloned().collect();
        self.gui_models(screen, &rest);
    }

    /// At the end of the stream (`last`) or at marks with models: the run of GUI commands so
    /// far (in the current GUI pass, or a new one after models), then the models anchored to
    /// `pending`, in their list order. Marks without models do not split the run.
    fn gui_run(&mut self, screen: [i32; 2], run: &mut Vec<GuiCommand>, pending: &mut Vec<GuiAnchor>, models: &[GuiModel], last: bool) {
        let here: Vec<GuiModel> = models.iter().filter(|g| g.anchor.is_some_and(|a| pending.contains(&a))).cloned().collect();
        pending.clear();
        if here.is_empty() && !last {
            return;
        }
        if !run.is_empty() {
            if self.out.passes.last().is_some_and(|p| p.kind != PassKind::Gui) {
                self.gui_state();
                self.begin(PassKind::Gui, (0x004b_b075, 0x004b_b080), None);
            }
            self.emit(0x0065_0980, Geometry::Gui(std::mem::take(run)));
        }
        self.gui_models(screen, &here);
    }
}

/// The CubeShader constants set at `0x004ad684..0x004adb04`.
pub fn frame_uniforms(inputs: &RenderInputs, sky: &SkyColors, projection: D3dMatrix) -> CubeUniforms {
    let cam = &inputs.camera;
    let cpos = render_position(cam, cam.position);
    CubeUniforms {
        view: cam.view,
        projection,
        camera_position: cpos,
        light_direction: normalize(sun_direction(inputs.time_of_day_ms)),
        light_front: sky.light_front,
        light_back: sky.light_back,
        // The first setLight (0x004ad957) passes the back colour as ambient too.
        ambient: sky.light_back,
        darkness: [0.0, 0.0, 0.0, 1.0],
        fog_scale: if cam.fog_distance > 0.0 { 1.0 / cam.fog_distance } else { 0.0 },
        fog_gradient_translation: fog_gradient_translation(if inputs.mode == FrameMode::MapScreen {
            90.0
        } else {
            cam.pitch
        }),
        sky_color1: sky.sky1,
        sky_color2: sky.sky2,
        fog_color: sky.fog,
    }
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = ((v[0] * v[0] + v[1] * v[1] + v[2] * v[2]) as f64).sqrt() as f32;
    let k = 1.0 / l;
    [v[0] * k, v[1] * k, v[2] * k]
}

fn is_underwater(inputs: &RenderInputs) -> bool {
    inputs.camera_block_kind & 0x1f == 2
}

/// The sky fan (`0x004adb04..0x004adede`): four `CubeVertex` at z = 100 with face +Z and
/// colour (1, 0, 0, 1) (ignored by PS 02), world `T(-1, -1, 0)`, identity view, and a
/// "projection" that doubles x/y and puts z near 0.
fn sky_fan() -> (Geometry, D3dMatrix, D3dMatrix) {
    let v = |x: u8, y: u8| WorldVertex { position: [x, y, 100, 4], color_argb: 0xffff_0000 };
    let geometry = Geometry::WorldImmediate {
        topology: Topology::TriangleFan,
        vertices: vec![v(0, 0), v(0, 2), v(2, 2), v(2, 0)],
    };
    let world = translation([-1.0, -1.0, 0.0]);
    let proj = [
        [2.0, 0.0, 0.0, 0.0],
        [0.0, 2.0, 0.0, 0.0],
        [0.0, 0.0, 1.0e-4, -1.0e-5],
        [0.0, 0.0, 0.0, 1.0],
    ];
    (geometry, world, proj)
}

/// Stars (`0x004ae641..0x004aea83`): each star `(x, y, z, size)` is transformed by the star
/// matrix with a perspective divide; a fan of the centre (opaque white) and four corners at
/// `±size` (transparent white), the first corner repeated.
pub fn star_fan(star_matrix: &D3dMatrix, star: [f32; 4]) -> Vec<FixedVertex> {
    let m = star_matrix;
    let (x, y, z, size) = (star[0], star[1], star[2], star[3]);
    let px = y * m[1][0] + x * m[0][0] + z * m[2][0] + m[3][0];
    let py = y * m[1][1] + x * m[0][1] + z * m[2][1] + m[3][1];
    let pz = y * m[1][2] + x * m[0][2] + z * m[2][2] + m[3][2];
    let pw = y * m[1][3] + x * m[0][3] + z * m[2][3] + m[3][3];
    let k = 1.0 / pw;
    let c = [k * px, k * py, k * pz];
    let corner = |dx: f32, dy: f32| FixedVertex {
        position: [c[0] + size * dx, c[1] + size * dy, c[2] + size * 0.0],
        color_argb: 0x00ff_ffff,
        uv: [0.0, 0.0],
    };
    let first = corner(-1.0, -1.0);
    vec![
        FixedVertex { position: c, color_argb: 0xffff_ffff, uv: [0.0, 0.0] },
        first,
        corner(1.0, -1.0),
        corner(1.0, 1.0),
        corner(-1.0, 1.0),
        first,
    ]
}

/// Sun sprite (`0x004aeae0..0x004aed61`): the sun direction × 100 through the star matrix
/// (with perspective divide), a 40-unit quad with uv 0..1, colour
/// `ARGB(255, 255, min(50 + 250 d, 255), 200 d²)`.
pub fn sun_quad(star_matrix: &D3dMatrix, sun_dir: [f32; 3], daylight: f32) -> Vec<FixedVertex> {
    let m = star_matrix;
    let p = [sun_dir[0] * 100.0, sun_dir[1] * 100.0, sun_dir[2] * 100.0];
    let w = 1.0 / (m[0][3] * p[0] + m[1][3] * p[1] + m[2][3] * p[2] + m[3][3]);
    let c = [
        w * (m[1][0] * p[1] + p[0] * m[0][0] + m[2][0] * p[2] + m[3][0]),
        w * (m[0][1] * p[0] + m[1][1] * p[1] + m[2][1] * p[2] + m[3][1]),
        w * (m[0][2] * p[0] + m[1][2] * p[1] + m[2][2] * p[2] + m[3][2]),
    ];
    let mut g = daylight * 250.0 + 50.0;
    if g > 255.0 {
        g = 255.0;
    }
    let b = (daylight * 200.0 * daylight) as i32 as u32 & 0xff;
    let color = (((g as i32 as u32) | 0x00ff_ff00) << 8) | b;
    let v = |dx: f32, dy: f32, u: f32, vv: f32| FixedVertex {
        position: [c[0] + 40.0 * dx, c[1] + 40.0 * dy, c[2]],
        color_argb: color,
        uv: [u, vv],
    };
    vec![v(-1.0, -1.0, 0.0, 0.0), v(1.0, -1.0, 1.0, 0.0), v(1.0, 1.0, 1.0, 1.0), v(-1.0, 1.0, 0.0, 1.0)]
}

/// One cloud of the grid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CloudDraw {
    /// Cell indices `(cx, cy)`.
    pub cell: [i32; 2],
    /// World matrix, render space.
    pub world: D3dMatrix,
}

/// Clouds (`0x004b1527..0x004b1a6b`). Only when not white-clearing, `climateA > 0.01` and
/// `climateC < 0.01` at the player's block. The grid is 13 × 13 (i, j in -6..=6) cells of
/// 200 blocks around `camera / 200` (truncated), no time drift. A cell `(cx, cy)` has a
/// cloud when `(noise(cx, cy) + 1)/2 < 0.8 · climateA(cx, cy)` (the original samples the
/// climate at the *cell indices* as if they were blocks) and `climateA >= 0.3` at the
/// cloud's block `(200 cx, 200 cy + 100 (cx mod 2))`. Height: `max(baseHeight + 100 ·
/// mountainFactor, 0) + 170 + 50 · noise(17 cx, 19 cy)`; scale `7 + 2 · noise(15 cx, 23 cy)`;
/// rotation `90 (cx + cy)` degrees about z; the model is centred on its x/y size.
pub fn cloud_draws(inputs: &RenderInputs) -> Vec<CloudDraw> {
    let world = inputs.world;
    let px = (inputs.player_position[0] / FIXED_ONE) as i32;
    let py = (inputs.player_position[1] / FIXED_ONE) as i32;
    let mut out = Vec::new();
    if inputs.white_clear {
        return out;
    }
    if !(world.climate_a(px, py) > 0.01) {
        return out;
    }
    if !(0.01 > world.climate_c(px, py)) {
        return out;
    }
    let cam_x = (inputs.camera.position[0] / FIXED_ONE) as i32;
    let cam_y = (inputs.camera.position[1] / FIXED_ONE) as i32;
    for i in -6..=6 {
        for j in -6..=6 {
            let cx = cam_x / 200 + i;
            let cy = cam_y / 200 + j;
            let n = (world.value_noise_2d(cx as f64, cy as f64) + 1.0) * 0.5;
            if n >= world.climate_a(cx, cy) * 0.8 {
                continue;
            }
            let bx = cx.wrapping_mul(200);
            let by = (cx % 2 + cy.wrapping_mul(2)).wrapping_mul(100);
            if 0.3 > world.climate_a(bx, by) {
                continue;
            }
            let m = world.mountain_factor(bx, by);
            let h = world.base_height(bx, by);
            let mut z = ftol((h + m * 100.0) * 65536.0);
            if z < 0 {
                z = 0;
            }
            let lift = world.value_noise_2d((cx * 17) as f64, (cy * 19) as f64) * 50.0 + 170.0;
            z += ftol(lift * 65536.0);
            let pos = [bx as i64 * FIXED_ONE, by as i64 * FIXED_ONE, z];
            let mut w = IDENTITY;
            pre_translate(&mut w, render_position(&inputs.camera, pos));
            let s = world.value_noise_2d((cx * 15) as f64, (cy * 23) as f64) * 2.0 + 7.0;
            pre_scale(&mut w, [s, s, s]);
            pre_rotate_z(&mut w, ((cy + cx) * 90) as f32);
            let half = [inputs.cloud_model_size[0] as f32 * -0.5, inputs.cloud_model_size[1] as f32 * -0.5, 0.0];
            pre_translate(&mut w, half);
            out.push(CloudDraw { cell: [cx, cy], world: w });
        }
    }
    out
}

/// After-image offsets and alphas (`0x004b5c00..0x004b5d04`): five draws, k = 0..4, alpha
/// `(1 - (1 - k/5)) · 0.5`, translated by `velocity · 0.07 · -(1 - k/5)`.
pub fn afterimages(velocity: [f32; 3]) -> [([f32; 3], f32); 5] {
    let mut out = [([0.0; 3], 0.0); 5];
    for (k, slot) in out.iter_mut().enumerate() {
        let t = 1.0 - (k as f32) / 5.0;
        let alpha = (1.0 - t) * 0.5;
        let s = -t * 0.07;
        *slot = ([velocity[0] * s, velocity[1] * s, velocity[2] * s], alpha);
    }
    out
}

/// Build the frame. See the module documentation for the map to the original.
pub fn build_frame(inputs: &RenderInputs) -> FrameCommands {
    let underwater = is_underwater(inputs);
    let d = daylight(inputs.time_of_day_ms);
    let cam = &inputs.camera;
    let cam_block = [(cam.position[0] / FIXED_ONE) as i32, (cam.position[1] / FIXED_ONE) as i32];
    let sky = sky_colors(
        inputs.time_of_day_ms,
        SkyClimate::sample(inputs.world, cam_block[0], cam_block[1]),
        underwater,
    );
    let proj = projection(inputs.screen, cam.narrow_fov);
    let mut b = Builder::new(frame_uniforms(inputs, &sky, proj));

    // 0x004ac312..0x004ac392: clear, ZENABLE=1, ZFUNC=LESSEQUAL, BeginScene.
    let clear_color = if inputs.white_clear { 0xffff_ffff } else { CLEAR_COLOR };
    b.set(|s| {
        s.depth.test = true;
        s.depth.func = CompareFunc::LessEqual;
    });
    b.begin(
        PassKind::ClearFrame,
        (0x004a_c312, 0x004a_c394),
        Some(Clear { color_argb: Some(clear_color), depth: Some(1.0), stencil: Some(0) }),
    );

    if inputs.mode == FrameMode::GuiOnly {
        build_gui_only(&mut b, inputs);
        return b.out;
    }

    // 0x004ac4c7..0x004ad434: visible chunks (previous frame's planes). The emitter lights
    // of the same walk come with `inputs.lights` from the gatherer.
    let visible = collect_visible_chunks(inputs, &inputs.previous_frustum);

    // 0x004ad684..0x004adb04: constants. Material, white 0, alpha 1, CULLMODE=NONE.
    let material = world_material(d, underwater);
    b.draw.material = material;
    b.draw.white = 0.0;
    b.draw.alpha = 1.0;
    b.draw.shininess = 0.0;
    b.draw.light_set = 0;
    b.set(|s| s.cull = Cull::None);

    // 0x004adb04..0x004adede: sky fan, VS00+PS02, ZENABLE=0.
    let (fan, fan_world, fan_proj) = sky_fan();
    let saved_view = b.uniforms.view;
    b.uni(|u| {
        u.view = IDENTITY;
        u.projection = fan_proj;
    });
    b.set(|s| {
        s.program = Program::SKY;
        s.depth.test = false;
    });
    b.begin(PassKind::Sky, (0x004a_db04, 0x004a_dede), None);
    b.draw.world = fan_world;
    if !inputs.white_clear {
        b.emit(0x004a_dedc, fan);
    }

    if inputs.mode == FrameMode::MapScreen {
        build_map_screen(&mut b, inputs);
        return b.out;
    }

    // 0x004ae370..0x004ae641: fixed function, lighting off, FVF 0x42, no texture.
    let sm = star_matrix(cam);
    b.set(|s| s.program = Program::FIXED_COLOR);
    b.fixed_transforms = [IDENTITY, IDENTITY, proj];
    b.begin(PassKind::Stars, (0x004a_e370, 0x004a_ea83), None);
    if 0.5 > d && !underwater {
        for star in &inputs.stars {
            let geometry = Geometry::FixedImmediate {
                topology: Topology::TriangleFan,
                vertices: star_fan(&sm, *star),
                texture: None,
                transforms: b.fixed_transforms,
            };
            b.emit(0x004a_ea5e, geometry);
        }
    }

    // 0x004aea83..0x004aed63: sun, FVF 0x142, texture, CLAMP.
    b.set(|s| {
        s.program = Program::FIXED_TEXTURED;
        s.sampler0 = SamplerState { address_u: AddressMode::Clamp, address_v: AddressMode::Clamp, ..s.sampler0 };
    });
    b.begin(PassKind::Sun, (0x004a_ea83, 0x004a_ed63), None);
    let geometry = Geometry::FixedImmediate {
        topology: Topology::TriangleFan,
        vertices: sun_quad(&sm, sun_direction(inputs.time_of_day_ms), d),
        texture: Some(inputs.sun_texture),
        transforms: b.fixed_transforms,
    };
    b.emit(0x004a_ed61, geometry);

    // 0x004aed63..0x004aee2e: bind VS00+PS01, sky colours, save projection, ZENABLE=1,
    // ALPHABLENDENABLE=1, SRCALPHA/INVSRCALPHA.
    b.uni(|u| {
        u.view = saved_view;
        u.projection = proj;
    });
    b.out.projection = Some(proj);
    b.set(|s| {
        s.program = Program::WORLD;
        s.depth.test = true;
        s.blend.enabled = true;
        s.blend.src = BlendFactor::SrcAlpha;
        s.blend.dst = BlendFactor::InvSrcAlpha;
    });
    // 0x004aee2e..0x004af353: planes for the next frame and for this frame's later tests.
    let planes = frustum_planes(&cam.view, &proj);
    b.out.frustum_planes = Some(planes);

    // 0x004af353..0x004b142f: the gathered lights (emitters, then dynamic ones, sorted near
    // → far by the gatherer's `std::sort 0x004abae0`), assigned to the chunks they touch,
    // and the first 16 as the global set.
    let all_lights: Vec<GatheredLight> = inputs
        .lights
        .iter()
        .map(|l| GatheredLight {
            world: l.position,
            light: PointLight { position: render_position(cam, l.position), radius: l.radius, color: l.color },
        })
        .collect();
    let chunk_sets = assign_lights_to_chunks(&inputs.window, inputs.chunks.len(), &all_lights);
    let global = LightSet {
        lights: all_lights.iter().take(LIGHT_SET_CAPACITY).map(|g| g.light).collect(),
    };
    let global_set = b.light_set(global);

    // 0x004b142f..0x004b1a6b: clouds. VS00+PS05, darkness = (sky1 + sky2)/2 with w = d,
    // fog ×1.5, material (1, 1, 1, d), light (front, front, dir, ambient).
    let dark = [
        (sky.sky1[0] + sky.sky2[0]) * 0.5,
        (sky.sky1[1] + sky.sky2[1]) * 0.5,
        (sky.sky1[2] + sky.sky2[2]) * 0.5,
        d,
    ];
    b.set(|s| s.program = Program::CLOUD);
    b.uni(|u| {
        u.darkness = dark;
        if cam.fog_distance * 1.5 > 0.0 {
            u.fog_scale = 1.0 / (cam.fog_distance * 1.5);
        }
        u.light_front = sky.light_front;
        u.light_back = sky.light_front;
        u.ambient = sky.ambient;
    });
    b.draw.material = [1.0, 1.0, 1.0, d];
    b.begin(PassKind::Clouds, (0x004b_142f, 0x004b_1a6b), None);
    for c in cloud_draws(inputs) {
        b.draw.world = c.world;
        b.emit(0x004b_1a42, Geometry::Model { model: inputs.cloud_model });
    }

    // 0x004b1a6b..0x004b1b69: bind, light (front, back, dir, ambient), fog, darkness 0,
    // CULLMODE=CW.
    b.set(|s| {
        s.program = Program::WORLD;
        s.cull = Cull::Cw;
    });
    b.uni(|u| {
        u.light_front = sky.light_front;
        u.light_back = sky.light_back;
        u.ambient = sky.ambient;
        if cam.fog_distance > 0.0 {
            u.fog_scale = 1.0 / cam.fog_distance;
        }
        u.darkness = [0.0, 0.0, 0.0, 1.0];
    });
    // Terrain (skipped as a whole when white-cleared: the visible list is empty).
    b.begin(PassKind::Terrain, (0x004b_1a6b, 0x004b_21ac), None);
    let chunk_set_ids: Vec<usize> = chunk_sets.into_iter().map(|s| b.light_set(s)).collect();
    for &(idx, _) in &visible {
        let chunk = &inputs.chunks[idx];
        b.draw.material = material;
        b.draw.light_set = chunk_set_ids[idx];
        b.draw.alpha = 1.0;
        for (bi, buf) in chunk.buffers.iter().enumerate() {
            if !buf.has_opaque {
                continue;
            }
            b.draw.world = chunk_world(cam, chunk.coords, buf.z_base);
            b.emit(
                0x004b_1d9a,
                Geometry::Chunk {
                    chunk: idx,
                    buffer: bi,
                    index_buffer: ChunkIndexBuffer::Opaque,
                    vertex_count: buf.vertex_count,
                    primitive_count: buf.opaque_primitives,
                },
            );
        }
        if let Some(props) = inputs.chunk_props.get(idx) {
            for p in props {
                b.model(p);
            }
        }
    }

    // 0x004b21ac..0x004b35d4: global lights, objects.
    b.draw.light_set = global_set;
    b.begin(PassKind::Objects, (0x004b_21ac, 0x004b_35d4), None);
    for o in &inputs.objects {
        b.model(o);
    }

    // 0x004b35d4..0x004b8bc7: creatures in the gatherer's order (far → near).
    b.begin(PassKind::Creatures, (0x004b_35d4, 0x004b_8bc7), None);
    let by_id: std::collections::HashMap<i64, usize> =
        inputs.creatures.iter().enumerate().map(|(i, c)| (c.id, i)).collect();
    let mut ghosts = Vec::new();
    for id in &inputs.creature_order {
        let Some(&ci) = by_id.get(id) else { continue };
        let c = &inputs.creatures[ci];
        // 0x004b4e1b: `0x0047f760(position +0x10, margin +0x88 (height), esp+0xbc)`. Every
        // creature, the local player included; the body, its after-images (0x004b5cf3) and the
        // ghost deferral (0x004b5d53) all sit behind this one test.
        if !sphere_visible(&planes, cam, c.position, c.height, c.cull_distance) {
            continue;
        }
        b.draw.white = c.flash;
        if c.afterimage.active() {
            b.set(|s| s.depth.write = false);
            for (offset, alpha) in afterimages(c.velocity) {
                for part in &c.parts {
                    let mut p = part.clone();
                    p.world = mat_mul(&part.world, &translation(offset));
                    p.alpha = alpha;
                    b.model(&p);
                }
            }
            b.set(|s| s.depth.write = true);
        }
        if c.ghost > 0.0 {
            ghosts.push(ci);
            continue;
        }
        for part in &c.parts {
            let mut p = part.clone();
            p.alpha = 1.0;
            b.model(&p);
        }
    }
    b.draw.white = 0.0;
    b.draw.alpha = 1.0;
    // Effects, particles (double-sided cubes), projectiles, in the gatherer's order.
    for e in &inputs.creature_extras {
        b.model(e);
    }

    // 0x004b8bc7..0x004b9993: ground decals and blob shadows, fixed function textured,
    // ZWRITE=0, sampler 0 ADDRESSU/V = BORDER (4) with the default border colour 0.
    b.set(|s| {
        s.program = Program::FIXED_TEXTURED;
        s.depth.write = false;
        s.sampler0.address_u = AddressMode::Border;
        s.sampler0.address_v = AddressMode::Border;
    });
    b.fixed_transforms = [IDENTITY, cam.view, proj];
    b.begin(PassKind::Shadows, (0x004b_8bc7, 0x004b_9993), None);
    for q in &inputs.shadows {
        let geometry = Geometry::FixedImmediate {
            topology: Topology::TriangleFan,
            vertices: q.to_vec(),
            texture: Some(inputs.shadow_texture),
            transforms: b.fixed_transforms,
        };
        b.emit(0x004b_9251, geometry);
    }

    // 0x004b9993..0x004ba13e: ribbons, untextured, CULLMODE=NONE, strips of 30.
    b.set(|s| {
        s.program = Program::FIXED_COLOR;
        s.cull = Cull::None;
    });
    b.begin(PassKind::Ribbons, (0x004b_9993, 0x004b_a13e), None);
    for r in &inputs.ribbons {
        let geometry = Geometry::FixedImmediate {
            topology: Topology::TriangleStrip,
            vertices: r.clone(),
            texture: None,
            transforms: b.fixed_transforms,
        };
        b.emit(0x004b_9f6f, geometry);
    }

    // 0x004ba13e..0x004ba432: water. Alpha 1, VS00+PS03, ZENABLE=1, ZWRITE=1, CULL NONE,
    // material, visible chunks far → near, chunk lights, second index buffer.
    b.draw.alpha = 1.0;
    b.draw.material = material;
    b.set(|s| {
        s.program = Program::WATER;
        s.depth.test = true;
        s.depth.write = true;
        s.cull = Cull::None;
    });
    b.begin(PassKind::Water, (0x004b_a13e, 0x004b_a432), None);
    for &(idx, _) in visible.iter().rev() {
        let chunk = &inputs.chunks[idx];
        for (bi, buf) in chunk.buffers.iter().enumerate() {
            if !buf.has_water {
                continue;
            }
            b.draw.world = chunk_world(cam, chunk.coords, buf.z_base);
            b.draw.light_set = chunk_set_ids[idx];
            b.emit(
                0x004b_a3a4,
                Geometry::Chunk {
                    chunk: idx,
                    buffer: bi,
                    index_buffer: ChunkIndexBuffer::Water,
                    vertex_count: buf.vertex_count,
                    primitive_count: buf.water_primitives,
                },
            );
        }
    }
    // 0x004ba432..0x004ba462: ZWRITE=1, CULLMODE=CW, VS00+PS01.
    b.set(|s| {
        s.depth.write = true;
        s.cull = Cull::Cw;
        s.program = Program::WORLD;
    });

    // 0x004ba462..0x004ba5d0: the far list, far → near, faded (props 0x004bd160, statics
    // 0x004be760). No light or material call in between: the point lights are still those
    // of the last water draw.
    b.begin(PassKind::FarObjects, (0x004b_a462, 0x004b_a5d0), None);
    for o in &inputs.far_objects {
        b.model(o);
    }

    // 0x004ba5d0..0x004ba9fc: ghosts: per creature a depth-only draw then a colour draw
    // with alpha 1 - 0.75·ghost, then alpha 1.
    b.draw.light_set = global_set;
    b.begin(PassKind::Ghosts, (0x004b_a5d0, 0x004b_a9fc), None);
    for ci in ghosts {
        let c = &inputs.creatures[ci];
        b.draw.white = c.flash;
        b.set(|s| s.color_write = 0);
        for part in &c.parts {
            let mut p = part.clone();
            p.alpha = 1.0;
            b.model(&p);
        }
        b.set(|s| s.color_write = 0xf);
        for part in &c.parts {
            let mut p = part.clone();
            p.alpha = 1.0 - c.ghost * 0.75;
            b.model(&p);
        }
    }
    b.draw.white = 0.0;
    b.draw.alpha = 1.0;
    b.draw.material = [1.0; 4];

    // 0x004ba9fc..0x004baa95: Clear(ZBUFFER) before the overlays.
    b.begin(
        PassKind::ClearDepth,
        (0x004b_a9fc, 0x004b_aa95),
        Some(Clear { color_argb: None, depth: Some(1.0), stencil: None }),
    );

    // 0x004baa95..0x004bb075: minimap (HUD and engine root visible), then its fixed lights.
    if inputs.hud_visible {
        b.begin(PassKind::Minimap, (0x004b_aa95, 0x004b_b075), None);
        b.map(0x004b_abaf, minimap_view(inputs), inputs.map.as_ref());
        b.set_light([1.0, 1.0, 1.0, 1.0], [0.2, 0.3, 0.4, 1.0], [0.5, 0.4, -0.6], [0.4, 0.4, 0.4, 1.0]);
        for mk in &inputs.minimap_markers {
            b.model(mk);
        }
        // 0x004bac91..0x004bb039: the Tab quick-item wheel.
        if !inputs.quick_wheel.is_empty() {
            b.draw.light_set = 0;
            for g in &inputs.quick_wheel {
                b.gui_model(inputs.screen, g);
            }
        }
    }

    // 0x004bb075: the GUI, the widgets' item models at their slot-1 points.
    b.gui_with_models(inputs.screen, &inputs.gui, &inputs.gui_models);
    b.gui_creatures(&inputs.gui_creatures);

    // 0x004bb080..0x004bbb1a: HUD models when the HUD is visible: ZENABLE=1, bind,
    // Clear(ZBUFFER), the HUD light (0x004bb259: `vec3f(0, 0.5, -1).normalized()`, which
    // the setter normalises again), the 16 point lights zeroed (0x004bb32a), then
    // 0x00476660 per model.
    if inputs.hud_visible {
        b.set(|s| {
            s.depth.test = true;
            s.program = Program::WORLD;
        });
        b.begin(
            PassKind::HudModels,
            (0x004b_b080, 0x004b_bb1a),
            Some(Clear { color_argb: None, depth: Some(1.0), stencil: None }),
        );
        b.set_light([1.0, 1.0, 1.0, 1.0], [0.2, 0.3, 0.4, 1.0], normalize([0.0, 0.5, -1.0]), [0.4, 0.4, 0.4, 1.0]);
        b.draw.light_set = 0;
        for h in &inputs.hud_models {
            b.hud_model(inputs.screen, h);
        }
    }
    b.out
}

/// Map-screen arguments (`0x004adede..0x004ae004`): centre = player + pan·65536
/// (truncated), centre pixel `(w/2, h/2)`, rotation `(min(-pitch, -110), 0, -yaw)`,
/// zoom `GC+0x1c4`, radius 16, full map.
pub fn map_screen_view(inputs: &RenderInputs) -> MapView {
    let cam = &inputs.camera;
    let mut center = inputs.player_position;
    for (c, p) in center.iter_mut().zip(inputs.map_pan) {
        *c += ftol(p * 65536.0);
    }
    let mut tilt = -110.0f32;
    if -cam.pitch <= -110.0 {
        tilt = -cam.pitch;
    }
    MapView {
        center,
        center_pixel: [
            ((inputs.screen[0] as f32) * 0.5) as i32,
            ((inputs.screen[1] as f32) * 0.5) as i32,
        ],
        viewport: inputs.screen,
        rotation: [tilt, 0.0, -cam.yaw],
        zoom: inputs.map_zoom,
        frame_ms: inputs.frame_ms,
        radius: 16,
        full: true,
    }
}

/// Minimap arguments (`0x004baac2..0x004babaf`): centre = player, centre pixel
/// `(w - 240, h - 170)`, rotation `(-120, 0, -yaw)`; zoom 0.025 with radius 0 when the
/// map zoom is below 0.35, 1.6/0 at 2 or more, else 0.8/1.
pub fn minimap_view(inputs: &RenderInputs) -> MapView {
    let z = inputs.map_zoom;
    let (zoom, radius) = if 0.35 > z {
        (0.025, 0)
    } else if z >= 2.0 {
        (1.6, 0)
    } else {
        (0.8, 1)
    };
    MapView {
        center: inputs.player_position,
        center_pixel: [inputs.screen[0] - 0xf0, inputs.screen[1] - 0xaa],
        viewport: inputs.screen,
        rotation: [-120.0, 0.0, -inputs.camera.yaw],
        zoom,
        frame_ms: inputs.frame_ms,
        radius,
        full: false,
    }
}

/// World matrix of a chunk buffer (`0x004b1c7c..0x004b1d1d`): translate by
/// `(32·cx + offset_x, 32·cy + offset_y)` blocks (via fixed point) and `z_base`.
pub fn chunk_world(camera: &CameraInputs, coords: [i32; 2], z_base: i32) -> D3dMatrix {
    let x = ((coords[0] as i64 * CHUNK_BLOCKS * FIXED_ONE + camera.render_offset[0]) as f32) * INV_FIXED;
    let y = ((coords[1] as i64 * CHUNK_BLOCKS * FIXED_ONE + camera.render_offset[1]) as f32) * INV_FIXED;
    let mut m = IDENTITY;
    pre_translate(&mut m, [x, y, z_base as f32]);
    m
}

/// `0x004ad43a..0x004ad67f`: only the GUI (with the in-game panels hidden by the caller).
fn build_gui_only(b: &mut Builder, inputs: &RenderInputs) {
    b.gui_with_models(inputs.screen, &inputs.gui, &inputs.gui_models);
    b.gui_creatures(&inputs.gui_creatures);
}

/// `0x004adede..0x004ae279`: the map screen. `0x005fc1b0`, material 1, fixed light, depth
/// clear, the compass (`0x00476660`), the GUI.
fn build_map_screen(b: &mut Builder, inputs: &RenderInputs) {
    b.set(|s| s.program = Program::WORLD);
    b.begin(PassKind::MapScreen, (0x004a_dede, 0x004a_e279), None);
    b.map(0x004a_e004, map_screen_view(inputs), inputs.map.as_ref());
    // 0x004ae009..0x004ae163: setMaterialColor(white), setLight of 0x004ae15e (direction
    // `(0, 0.5, -1)·k`, `k = 1/sqrt(1.25)` inline, normalised again by the setter).
    b.draw.material = [1.0; 4];
    let k = 1.0 / (1.25f64.sqrt() as f32);
    b.set_light([1.0, 1.0, 1.0, 1.0], [0.2, 0.3, 0.4, 1.0], [k * 0.0, k * 0.5, k * -1.0], [0.4, 0.4, 0.4, 1.0]);
    // Clear(ZBUFFER), then the compass (0x004ae21b). The point lights stay zeroed by the map.
    b.begin(
        PassKind::HudModels,
        (0x004a_e163, 0x004a_e220),
        Some(Clear { color_argb: None, depth: Some(1.0), stencil: None }),
    );
    b.draw.light_set = 0;
    if let Some(h) = &inputs.map_compass {
        b.hud_model(inputs.screen, h);
    }
    b.gui_with_models(inputs.screen, &inputs.gui, &inputs.gui_models);
    b.gui_creatures(&inputs.gui_creatures);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera() -> CameraInputs {
        let mut view = IDENTITY;
        // Look along +y from above: a simple LH view where +y is forward.
        view[1] = [0.0, 0.0, 1.0, 0.0];
        view[2] = [0.0, 1.0, 0.0, 0.0];
        CameraInputs {
            position: [0, 0, 0],
            render_offset: [0, 0],
            view,
            sky_view: view,
            pitch: 80.0,
            yaw: 0.0,
            narrow_fov: false,
            fog_distance: 100.0,
        }
    }

    fn inputs(world: &dyn WorldSampler) -> RenderInputs<'_> {
        RenderInputs {
            mode: FrameMode::World,
            screen: [800, 600],
            white_clear: false,
            time_of_day_ms: 43_200_000,
            clock_ms: 0,
            frame_ms: 16,
            camera: camera(),
            camera_block_kind: 0,
            player_position: [0, 0, 0],
            previous_frustum: [[0.0, 0.0, 0.0, 1.0]; 6],
            window: ChunkWindow { origin: [0, 0], size: 2 },
            chunks: (0..4)
                .map(|i| ChunkInput {
                    coords: [i % 2, i / 2],
                    has_buffers: true,
                    aabb_min: [(i % 2) as i64 * 32 * FIXED_ONE, (i / 2) as i64 * 32 * FIXED_ONE, 0],
                    aabb_max: [((i % 2) as i64 * 32 + 32) * FIXED_ONE, ((i / 2) as i64 * 32 + 32) * FIXED_ONE, 64 * FIXED_ONE],
                    center: [((i % 2) as i64 * 32 + 16) * FIXED_ONE, ((i / 2) as i64 * 32 + 16) * FIXED_ONE, 32 * FIXED_ONE],
                    buffers: vec![ChunkBufferInput {
                        z_base: 0,
                        vertex_count: 8,
                        opaque_primitives: 4,
                        water_primitives: 2,
                        has_opaque: true,
                        has_water: true,
                    }],
                })
                .collect(),
            chunk_props: vec![],
            lights: vec![],
            stars: vec![[0.0, 1.0, 0.5, 0.01]],
            sun_texture: 1,
            shadow_texture: 2,
            cloud_model: 3,
            cloud_model_size: [20, 20],
            objects: vec![],
            creatures: vec![],
            creature_order: vec![],
            creature_extras: vec![],
            shadows: vec![],
            ribbons: vec![],
            far_objects: vec![],
            map_pan: [0.0; 3],
            map_zoom: 1.0,
            minimap_markers: vec![],
            quick_wheel: vec![],
            gui: vec![],
            hud_visible: false,
            gui_models: vec![],
            gui_creatures: vec![],
            hud_models: vec![],
            map_compass: None,
            map: None,
            world,
        }
    }

    #[test]
    fn world_pass_order() {
        let w = FlatWorld;
        let f = build_frame(&inputs(&w));
        assert_eq!(
            f.pass_kinds(),
            vec![
                PassKind::ClearFrame,
                PassKind::Sky,
                PassKind::Stars,
                PassKind::Sun,
                PassKind::Clouds,
                PassKind::Terrain,
                PassKind::Objects,
                PassKind::Creatures,
                PassKind::Shadows,
                PassKind::Ribbons,
                PassKind::Water,
                PassKind::FarObjects,
                PassKind::Ghosts,
                PassKind::ClearDepth,
                PassKind::Gui,
            ]
        );
        let clear = f.pass(PassKind::ClearFrame).unwrap().clear.unwrap();
        assert_eq!(clear.color_argb, Some(CLEAR_COLOR));
        assert_eq!(clear.depth, Some(1.0));
    }

    #[test]
    fn states_and_blending() {
        let w = FlatWorld;
        let f = build_frame(&inputs(&w));
        let sky = f.pass(PassKind::Sky).unwrap();
        assert_eq!(sky.state.program, Program::SKY);
        assert!(!sky.state.depth.test);
        assert_eq!(sky.state.cull, Cull::None);
        assert_eq!(sky.draws.len(), 1);

        let terrain = f.pass(PassKind::Terrain).unwrap();
        assert_eq!(terrain.state.program, Program::WORLD);
        assert!(terrain.state.depth.test);
        assert_eq!(terrain.state.depth.func, CompareFunc::LessEqual);
        assert_eq!(terrain.state.cull, Cull::Cw);
        assert_eq!(terrain.state.blend, BlendState::ALPHA);
        assert_eq!(terrain.draws.len(), 4);

        let water = f.pass(PassKind::Water).unwrap();
        assert_eq!(water.state.program, Program::WATER);
        assert!(water.state.depth.write);
        assert_eq!(water.state.cull, Cull::None);
        assert!(water.state.blend.enabled);
        // Far to near: the reverse of the terrain order.
        let t: Vec<_> = terrain.draws.iter().map(|d| d.geometry.clone()).collect();
        let wa: Vec<usize> = water
            .draws
            .iter()
            .map(|d| match d.geometry {
                Geometry::Chunk { chunk, .. } => chunk,
                _ => unreachable!(),
            })
            .collect();
        let te: Vec<usize> = t
            .iter()
            .map(|g| match g {
                Geometry::Chunk { chunk, .. } => *chunk,
                _ => unreachable!(),
            })
            .rev()
            .collect();
        assert_eq!(wa, te);

        let gui = f.pass(PassKind::Gui).unwrap();
        assert_eq!(gui.state.program, Program::GUI);
        assert!(!gui.state.depth.test);
        assert_eq!(gui.state.blend.src_alpha, BlendFactor::One);
        assert_eq!(gui.state.blend.dst_alpha, BlendFactor::InvSrcAlpha);

        let sun = f.pass(PassKind::Sun).unwrap();
        assert_eq!(sun.state.program, Program::FIXED_TEXTURED);
        assert_eq!(sun.state.sampler0.address_u, AddressMode::Clamp);
        // Noon: no stars.
        assert!(f.pass(PassKind::Stars).unwrap().draws.is_empty());
    }

    #[test]
    fn ghost_prepass_and_afterimages() {
        let w = FlatWorld;
        let mut inp = inputs(&w);
        let part = ModelDraw {
            origin: 0,
            model: 9,
            world: translation([0.0, 10.0, 0.0]),
            material: [1.0; 4],
            alpha: 1.0,
            double_sided: false,
            shininess: 0.0,
            set_white: None,
            mirrored: false,
        };
        inp.creatures.push(CreatureInput {
            id: 1,
            position: [0, 10 * FIXED_ONE, 0],
            height: 2.0,
            ghost: 0.5,
            parts: vec![part.clone()],
            ..Default::default()
        });
        inp.creatures.push(CreatureInput {
            id: 2,
            position: [0, 12 * FIXED_ONE, 0],
            height: 2.0,
            afterimage: AfterimageFields { mode: 0x30, ..Default::default() },
            parts: vec![part],
            ..Default::default()
        });
        inp.creature_order = vec![1, 2];
        let f = build_frame(&inp);
        let ghosts = f.pass(PassKind::Ghosts).unwrap();
        assert_eq!(ghosts.draws.len(), 2);
        assert_eq!(ghosts.draws[0].state.color_write, 0);
        assert_eq!(ghosts.draws[1].state.color_write, 0xf);
        assert!((ghosts.draws[1].draw.alpha - (1.0 - 0.5 * 0.75)).abs() < 1e-6);
        let cr = f.pass(PassKind::Creatures).unwrap();
        // Five after-images without depth writes, then the body.
        assert_eq!(cr.draws.len(), 6);
        assert!(cr.draws[..5].iter().all(|d| !d.state.depth.write));
        assert!(cr.draws[5].state.depth.write);
        assert!((cr.draws[4].draw.alpha - 0.4).abs() < 1e-6);
    }

    #[test]
    fn gui_only_and_map_paths() {
        let w = FlatWorld;
        let mut inp = inputs(&w);
        inp.mode = FrameMode::GuiOnly;
        assert_eq!(build_frame(&inp).pass_kinds(), vec![PassKind::ClearFrame, PassKind::Gui]);
        inp.mode = FrameMode::MapScreen;
        let f = build_frame(&inp);
        match &f.pass(PassKind::MapScreen).unwrap().draws[0].geometry {
            Geometry::Map(m) => {
                assert!(m.full);
                assert_eq!(m.center_pixel, [400, 300]);
                assert_eq!(m.radius, 16);
                assert_eq!(m.rotation[0], -110.0);
            }
            g => panic!("{g:?}"),
        }
        assert_eq!(
            f.pass_kinds(),
            vec![PassKind::ClearFrame, PassKind::Sky, PassKind::MapScreen, PassKind::HudModels, PassKind::Gui]
        );
        // The map screen uses the fixed gradient pitch 90.
        let u = &f.uniforms[f.pass(PassKind::Sky).unwrap().draws[0].uniforms];
        assert!((u.fog_gradient_translation - (90.0 - 80.0) / -30.0).abs() < 1e-6);
    }

    #[test]
    fn point_lights_upload_ten() {
        let set = LightSet {
            lights: (0..16)
                .map(|i| PointLight { position: [i as f32, 0.0, 0.0], radius: 1.0, color: [1.0, 0.5, 0.25] })
                .collect(),
        };
        let (pos, col) = set.upload();
        assert_eq!(pos.len(), 10);
        assert_eq!(pos[9], [9.0, 0.0, 0.0, 1.0]);
        assert_eq!(col[0], [1.0, 0.5, 0.25, 0.0]);
    }

    #[test]
    fn lights_assigned_to_touched_chunks() {
        let window = ChunkWindow { origin: [0, 0], size: 2 };
        let l = GatheredLight {
            world: [30 * FIXED_ONE, 10 * FIXED_ONE, 0],
            light: PointLight { position: [30.0, 10.0, 0.0], radius: 15.0, color: [1.0; 3] },
        };
        let sets = assign_lights_to_chunks(&window, 4, &[l]);
        // x range 15..45 → chunks 0 and 1; y range -5..25 → chunk 0 (and -0 → 0).
        assert_eq!(sets[0].lights.len(), 1);
        assert_eq!(sets[1].lights.len(), 1);
        assert_eq!(sets[2].lights.len(), 0);
        // Capacity 16.
        let many: Vec<_> = std::iter::repeat_n(l, 20).collect();
        assert_eq!(assign_lights_to_chunks(&window, 4, &many)[0].lights.len(), 16);
    }

    #[test]
    fn daylight_and_sun() {
        assert!((daylight(43_200_000) - 1.0).abs() < 1e-6);
        assert!(daylight(0).abs() < 1e-6);
        let s = sun_direction(43_200_000);
        assert!(s[1].abs() < 1e-6 && (s[2] - 1.0).abs() < 1e-6);
        let c = sky_colors(43_200_000, SkyClimate::default(), false);
        // Noon, climate 0: the `f8 > 0.75` branch with the `a < 0.4` blend applied.
        assert_eq!(c.sky1, [0.6, 0.8, 1.0, 1.0]);
        assert_eq!(c.ambient, [0.3, 0.4, 0.6, 1.0]);
        let u = sky_colors(43_200_000, SkyClimate::default(), true);
        assert_eq!(u.sky1, [0.1, 0.15, 1.0, 1.0]);
    }

    struct CloudyWorld;
    impl WorldSampler for CloudyWorld {
        fn climate_a(&self, _: i32, _: i32) -> f32 {
            1.0
        }
        fn climate_b(&self, _: i32, _: i32) -> f32 {
            0.5
        }
        fn climate_c(&self, _: i32, _: i32) -> f32 {
            0.0
        }
        fn climate_d(&self, _: i32, _: i32) -> f32 {
            0.0
        }
        fn mountain_factor(&self, _: i32, _: i32) -> f32 {
            0.0
        }
        fn base_height(&self, _: i32, _: i32) -> f32 {
            10.0
        }
        fn value_noise_2d(&self, _: f64, _: f64) -> f32 {
            0.0
        }
    }

    #[test]
    fn cloud_grid_is_13_by_13() {
        let w = CloudyWorld;
        let inp = inputs(&w);
        let clouds = cloud_draws(&inp);
        assert_eq!(clouds.len(), 169);
        // Cell (0, 0): height 10 + 170, scale 7, rotation 0, centred on a 20×20 model.
        let c = clouds.iter().find(|c| c.cell == [0, 0]).unwrap();
        assert!((c.world[3][2] - 180.0).abs() < 1e-3);
        assert!((c.world[0][0] - 7.0).abs() < 1e-5);
        assert!((c.world[3][0] - (-70.0)).abs() < 1e-3);
        let f = build_frame(&inp);
        let p = f.pass(PassKind::Clouds).unwrap();
        assert_eq!(p.state.program, Program::CLOUD);
        assert_eq!(p.state.cull, Cull::None);
        assert_eq!(p.draws.len(), 169);
        let u = &f.uniforms[p.draws[0].uniforms];
        assert!((u.fog_scale - 1.0 / 150.0).abs() < 1e-7);
    }

    #[test]
    fn culling_tests() {
        let cam = camera();
        let proj = projection([800, 600], false);
        let planes = frustum_planes(&cam.view, &proj);
        // In front (+y) is visible, behind is not.
        assert!(sphere_visible(&planes, &cam, [0, 20 * FIXED_ONE, 0], 1.0, 100.0));
        assert!(!sphere_visible(&planes, &cam, [0, -20 * FIXED_ONE, 0], 1.0, 100.0));
        // Too far.
        assert!(!sphere_visible(&planes, &cam, [0, 200 * FIXED_ONE, 0], 1.0, 100.0));
        assert!(box_visible(&planes, &cam, [0, 0, 0], [32 * FIXED_ONE, 32 * FIXED_ONE, 32 * FIXED_ONE], 100.0));
    }

    #[test]
    fn vs_constants_layout() {
        let w = FlatWorld;
        let f = build_frame(&inputs(&w));
        let t = f.pass(PassKind::Terrain).unwrap();
        let c = f.vs_constants(&t.draws[0]);
        // c32 material = (1, 1, 1, daylight 1), c39 white 0.
        assert_eq!(c[12], [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(c[19][0], 0.0);
        assert!((c[10][0] - 0.01).abs() < 1e-7);
    }

    fn model(id: ModelRef, y: f32) -> ModelDraw {
        ModelDraw::new(0x1234, id, translation([0.0, y, 0.0]), [1.0; 4])
    }

    #[test]
    fn lights_come_sorted_from_the_gatherer() {
        let w = FlatWorld;
        let mut inp = inputs(&w);
        // 20 lights in the given (already sorted) order: the global set is the first 16 as
        // given, nothing is re-sorted here.
        inp.lights = (0..20)
            .map(|i| PointLightSource { position: [i * FIXED_ONE, 0, 0], radius: 5.0, color: [1.0; 3] })
            .collect();
        let f = build_frame(&inp);
        let obj_set = f.light_sets.iter().find(|s| s.lights.len() == 16).expect("global set");
        assert_eq!(obj_set.lights[0].position, [0.0, 0.0, 0.0]);
        assert_eq!(obj_set.lights[15].position, [15.0, 0.0, 0.0]);
    }

    #[test]
    fn white_clear_skips_sky_terrain_and_water() {
        let w = FlatWorld;
        let mut inp = inputs(&w);
        inp.white_clear = true;
        let f = build_frame(&inp);
        assert_eq!(f.pass(PassKind::ClearFrame).unwrap().clear.unwrap().color_argb, Some(0xffff_ffff));
        assert!(f.pass(PassKind::Sky).unwrap().draws.is_empty());
        assert!(f.pass(PassKind::Terrain).unwrap().draws.is_empty());
        assert!(f.pass(PassKind::Water).unwrap().draws.is_empty());
    }

    #[test]
    fn far_objects_after_water_and_creature_order() {
        let w = FlatWorld;
        let mut inp = inputs(&w);
        inp.far_objects = vec![model(40, 30.0), model(41, 20.0)];
        let body = |id: i64, m: ModelRef, y: i64| CreatureInput {
            id,
            position: [0, y * FIXED_ONE, 0],
            height: 2.0,
            parts: vec![model(m, y as f32)],
            ..Default::default()
        };
        inp.creatures = vec![body(1, 50, 10), body(2, 51, 20), body(3, 52, 15)];
        inp.creature_order = vec![2, 3, 1, 99];
        let f = build_frame(&inp);
        let kinds = f.pass_kinds();
        let water = kinds.iter().position(|k| *k == PassKind::Water).unwrap();
        assert_eq!(kinds[water + 1], PassKind::FarObjects);
        let far: Vec<ModelRef> = f
            .pass(PassKind::FarObjects)
            .unwrap()
            .draws
            .iter()
            .map(|d| match d.geometry {
                Geometry::Model { model } => model,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(far, vec![40, 41]);
        let cr: Vec<ModelRef> = f
            .pass(PassKind::Creatures)
            .unwrap()
            .draws
            .iter()
            .map(|d| match d.geometry {
                Geometry::Model { model } => model,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(cr, vec![51, 52, 50]);
    }

    /// 0x004b4e1b: the body's sphere test `0x0047f760` takes its distance limit from
    /// `esp+0xbc` = `GC+0x1d0 · 0.3` (0x004ad9d1..0x004ada04), the range of the creature
    /// effects, not the chunk draw distance; the creature carries it as `cull_distance`.
    #[test]
    fn creature_body_uses_its_cull_distance() {
        let w = FlatWorld;
        let mut inp = inputs(&w);
        let body = |id: i64, m: ModelRef, y: i64, range: f32| CreatureInput {
            id,
            position: [0, y * FIXED_ONE, 0],
            height: 2.0,
            cull_distance: range,
            parts: vec![model(m, y as f32)],
            ..Default::default()
        };
        inp.creatures = vec![body(1, 50, 10, 5.0), body(2, 51, 10, 50.0)];
        inp.creature_order = vec![1, 2];
        let f = build_frame(&inp);
        let cr: Vec<ModelRef> = f
            .pass(PassKind::Creatures)
            .unwrap()
            .draws
            .iter()
            .map(|d| match d.geometry {
                Geometry::Model { model } => model,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(cr, vec![51]);
    }

    #[test]
    fn shininess_and_white_are_per_draw() {
        let w = FlatWorld;
        let mut inp = inputs(&w);
        let mut shiny = model(60, 10.0);
        shiny.shininess = 1.0;
        shiny.set_white = Some(0.5);
        let mut particle = model(61, 10.0);
        particle.double_sided = true;
        inp.creature_extras = vec![shiny, particle];
        let f = build_frame(&inp);
        let d = &f.pass(PassKind::Creatures).unwrap().draws;
        assert_eq!((d[0].draw.shininess, d[0].draw.white), (1.0, 0.5));
        assert_eq!((d[1].draw.shininess, d[1].draw.white), (0.0, 0.0));
        assert_eq!(d[1].state.cull, Cull::None);
        assert_eq!(d[0].state.cull, Cull::Cw);
    }

    #[test]
    fn mirrored_hand_culls_the_other_winding() {
        let w = FlatWorld;
        let mut inp = inputs(&w);
        let mut hand = model(70, 10.0);
        hand.mirrored = true;
        inp.creature_extras = vec![hand, model(71, 10.0)];
        let f = build_frame(&inp);
        let d = &f.pass(PassKind::Creatures).unwrap().draws;
        // 0x00423588 CCW for the mirrored draw, 0x004235bf CW after it.
        assert_eq!(d[0].state.cull, Cull::Ccw);
        assert_eq!(d[1].state.cull, Cull::Cw);
    }

    #[test]
    fn decals_use_border_addressing() {
        let w = FlatWorld;
        let mut inp = inputs(&w);
        let v = FixedVertex { position: [0.0; 3], color_argb: 0xff00_0000, uv: [0.0; 2] };
        inp.shadows = vec![[v; 4]];
        let f = build_frame(&inp);
        let s = f.pass(PassKind::Shadows).unwrap();
        assert_eq!(s.draws[0].state.sampler0.address_u, AddressMode::Border);
        assert_eq!(s.draws[0].state.sampler0.address_v, AddressMode::Border);
        assert!(!s.draws[0].state.depth.write);
    }

    #[test]
    fn quick_wheel_in_the_hud_branch_before_the_gui() {
        let w = FlatWorld;
        let mut inp = inputs(&w);
        inp.hud_visible = true;
        let g = GuiModel {
            origin: 0x004b_b01d,
            screen: [400.0, 100.0],
            rotation: [0.0, 0.0, 0.0],
            scale: 0.09,
            ring_turn: false,
            model: 5,
            model_size: [4, 8, 2],
            depth: 0.0,
            material: [1.0; 4],
            shininess: 0.0,
            spirits: vec![],
            spirit_model: 9,
            anchor: None,
        };
        inp.quick_wheel = vec![g];
        let f = build_frame(&inp);
        let p = f.pass(PassKind::Minimap).unwrap();
        let d = p.draws.iter().find(|d| d.origin == 0x004b_b01d).expect("wheel model drawn");
        assert!(d.state.depth.test);
        assert_eq!(d.draw.light_set, 0);
        // Not drawn without the HUD.
        inp.hud_visible = false;
        let f = build_frame(&inp);
        assert!(f.passes.iter().all(|p| p.draws.iter().all(|d| d.origin != 0x004b_b01d)));
    }

    fn test_gui_draw(tag: u32, mask_surface: Option<u32>) -> GuiCommand {
        GuiCommand::Draw(GuiDraw {
            vertices: tag..tag + 4,
            indices: 0..6,
            proj: IDENTITY,
            world_view: IDENTITY,
            deformation_enabled: false,
            widget_bind_matrix: IDENTITY,
            inverse_widget_bind_matrix: IDENTITY,
            mask_matrix: [[0.0; 4]; 2],
            texture_matrix: [[0.0; 4]; 2],
            normal_matrix: [[0.0; 4]; 2],
            widget_bind_pos: [0.0; 2],
            widget_bind_size: [0.0; 2],
            deformed_widget_pos: [0.0; 2],
            deformed_widget_size: [0.0; 2],
            aa_offset: 0.0,
            base_color: [1.0; 4],
            texture_enabled: false,
            filter: 0,
            texture_opacity: 1.0,
            texture_brightness: 0.0,
            texture_contrast: 1.0,
            texture_saturation: 1.0,
            texture: None,
            mask: None,
            mask_surface,
            subtract: false,
        })
    }

    fn test_gui_model(origin: u32, anchor: Option<GuiAnchor>) -> GuiModel {
        GuiModel {
            origin,
            screen: [400.0, 300.0],
            rotation: [0.0; 3],
            scale: 1.0,
            ring_turn: false,
            model: 5,
            model_size: [4, 8, 2],
            depth: 0.0,
            material: [1.0; 4],
            shininess: 0.0,
            spirits: vec![],
            spirit_model: 9,
            anchor,
        }
    }

    /// The GUI draw commands of a frame in order, as their `vertices.start` tags, with the
    /// model origins between them.
    fn gui_sequence(f: &FrameCommands) -> Vec<String> {
        let mut out = Vec::new();
        for p in &f.passes {
            for d in &p.draws {
                match &d.geometry {
                    Geometry::Gui(cmds) => {
                        for c in cmds {
                            match c {
                                GuiCommand::Draw(g) => out.push(format!("g{}", g.vertices.start)),
                                GuiCommand::BeginSurface { .. } => out.push("begin".into()),
                                GuiCommand::EndSurface => out.push("end".into()),
                                GuiCommand::WidgetMark(_) => out.push("mark".into()),
                                _ => out.push("other".into()),
                            }
                        }
                    }
                    Geometry::Model { .. } if p.kind == PassKind::GuiModels => out.push(format!("m{:x}", d.origin)),
                    _ => {}
                }
            }
        }
        out
    }

    /// `InventoryWidget::update` 0x004c2050 draws its cell models (0x004c2577, render state
    /// 7 = 1 around them) and only then the stack counts (`drawText` 0x00639b30 from
    /// 0x004c2862 on), all in the widget's slot 1 of `Node::render` 0x00632910; the cursor
    /// stack (0x004c595a) comes after the last text. A model anchored to a widget mark is
    /// drawn at that point of the GUI stream, so the GUI drawn after it (the counts, later
    /// widgets) covers it; the marks themselves are stripped.
    #[test]
    fn gui_models_at_their_widget_marks() {
        let w = FlatWorld;
        let mut inp = inputs(&w);
        let before = GuiAnchor { widget: 7, after_texts: false };
        let after = GuiAnchor { widget: 7, after_texts: true };
        inp.gui = vec![
            test_gui_draw(10, None),
            // A widget without models: no split.
            GuiCommand::WidgetMark(GuiAnchor { widget: 8, after_texts: false }),
            test_gui_draw(15, None),
            GuiCommand::WidgetMark(before),
            test_gui_draw(20, None), // the count text
            GuiCommand::WidgetMark(after),
            test_gui_draw(30, None), // a later widget
        ];
        inp.gui_models = vec![
            test_gui_model(0x004c_595a, Some(after)),
            test_gui_model(0x004c_2577, Some(before)),
            test_gui_model(0x004d_5d02, None),
            test_gui_model(0x0051_c6a7, Some(GuiAnchor { widget: 99, after_texts: false })),
        ];
        let f = build_frame(&inp);
        assert_eq!(gui_sequence(&f), ["g10", "g15", "m4c2577", "g20", "m4c595a", "g30", "m4d5d02"]);
        // The first GUI pass clears depth and stencil (beginFrame); the continuations do not,
        // and restore the GUI state after the models' depth test.
        let gui_passes: Vec<&Pass> = f.passes.iter().filter(|p| p.kind == PassKind::Gui).collect();
        assert_eq!(gui_passes.len(), 3);
        assert!(gui_passes[0].clear.is_some());
        assert!(gui_passes[1..].iter().all(|p| p.clear.is_none()));
        for p in &gui_passes {
            assert!(p.draws.iter().all(|d| !d.state.depth.test && d.state.program == Program::GUI));
        }
        for p in f.passes.iter().filter(|p| p.kind == PassKind::GuiModels) {
            assert!(p.clear.is_none());
            assert!(p.draws.iter().all(|d| d.state.depth.test && d.draw.light_set == 0));
        }
    }

    /// A mark inside a render surface (a clipping or blurred ancestor) cannot split the
    /// stream (the executor's surface stack is per `Geometry::Gui`): its models wait until
    /// the stream is back on the back buffer and past the level's composite (the masked draw
    /// of the clipping node or the blur's surface quads).
    #[test]
    fn gui_models_inside_a_surface_wait_for_the_composite() {
        let w = FlatWorld;
        let mut inp = inputs(&w);
        let a = GuiAnchor { widget: 3, after_texts: false };
        inp.gui = vec![
            test_gui_draw(10, None),
            GuiCommand::BeginSurface { surface: 1, clear_argb: Some(0) },
            GuiCommand::WidgetMark(a),
            test_gui_draw(20, None),
            GuiCommand::EndSurface,
            test_gui_draw(30, Some(1)),
            test_gui_draw(40, None),
        ];
        inp.gui_models = vec![test_gui_model(0x004c_2577, Some(a))];
        let f = build_frame(&inp);
        assert_eq!(gui_sequence(&f), ["g10", "begin", "g20", "end", "g30", "m4c2577", "g40"]);
    }

    #[test]
    fn gui_creatures_after_the_gui_with_their_own_camera() {
        let w = FlatWorld;
        let mut inp = inputs(&w);
        let mut view = IDENTITY;
        view[3] = [1.0, 2.0, 17.0, 1.0];
        let mut projection = IDENTITY;
        projection[2][3] = 1.0;
        let part = ModelDraw::new(0x0041_28f0, 7, translation([0.0, 0.0, 1.0]), [1.0; 4]);
        inp.gui_creatures = vec![GuiCreature {
            origin: 0x0042_69e1,
            view,
            projection,
            light_direction: [0.5, 1.0, 1.5],
            parts: vec![part.clone(), part],
        }];
        let f = build_frame(&inp);
        let last = f.passes.last().unwrap();
        assert_eq!(last.kind, PassKind::GuiModels);
        assert!(last.clear.is_none());
        assert_eq!(last.draws.len(), 2);
        let d = &last.draws[0];
        assert!(d.state.depth.test);
        assert_eq!(d.state.program, Program::WORLD);
        assert_eq!((d.draw.light_set, d.draw.alpha, d.draw.white), (0, 1.0, 0.0));
        let u = &f.uniforms[d.uniforms];
        assert_eq!(u.view, view);
        assert_eq!(u.projection, projection);
        assert_eq!(u.camera_position, [0.0; 3]);
        assert_eq!(u.fog_scale, 1.0 / 1.0e9);
        assert_eq!(u.ambient, [0.4, 0.4, 0.4, 1.0]);
        let k = 1.0 / 3.5f32.sqrt();
        assert!((u.light_direction[1] - k).abs() < 1e-6);
    }

    #[test]
    fn gui_models_after_the_gui() {
        let w = FlatWorld;
        let mut inp = inputs(&w);
        let g = GuiModel {
            origin: 0x004c_2577,
            screen: [400.0, 300.0],
            rotation: [0.0, 0.0, 0.0],
            scale: 2.0,
            ring_turn: false,
            model: 5,
            model_size: [4, 8, 2],
            depth: 0.0,
            material: [1.0; 4],
            shininess: 1.0,
            spirits: vec![([1.0, 2.0, 3.0], [1.0, 0.0, 0.0, 1.0])],
            spirit_model: 9,
            anchor: None,
        };
        inp.gui_models = vec![g.clone()];
        let f = build_frame(&inp);
        let kinds: Vec<PassKind> = f.passes.iter().map(|p| p.kind).collect();
        let gi = kinds.iter().position(|k| *k == PassKind::Gui).unwrap();
        assert_eq!(kinds[gi + 1], PassKind::GuiModels);
        let p = f.pass(PassKind::GuiModels).unwrap();
        assert!(p.clear.is_none());
        assert_eq!(p.draws.len(), 2);
        assert_eq!(p.draws[0].state.depth.func, CompareFunc::Less);
        assert!(p.draws[0].state.depth.test);
        assert_eq!((p.draws[0].draw.shininess, p.draws[0].draw.light_set), (1.0, 0));
        assert_eq!(p.draws[1].geometry, Geometry::Model { model: 9 });
        assert_eq!(p.draws[1].draw.material, [1.0, 0.0, 0.0, 1.0]);
        // No rotation: scale 2 / 8, centred, at view depth 1.
        let (m, _) = gui_model_matrices([800, 600], &g);
        assert_eq!(m[0][0], 0.25);
        assert_eq!(m[3], [-0.5, -1.0, 0.75, 1.0]);
        // The spirit cube at `world · T(1, 2, 3)`.
        assert_eq!(p.draws[1].draw.world[3], [-0.25, -0.5, 1.5, 1.0]);
        // `P · T`: the model centre (view (0, 0, 1)) lands on its pixel.
        let at = GuiModel { screen: [600.0, 150.0], ..g };
        let (_, proj) = gui_model_matrices([800, 600], &at);
        let c: Vec<f32> = (0..4).map(|j| proj[2][j] + proj[3][j]).collect();
        let (nx, ny) = (c[0] / c[3], c[1] / c[3]);
        assert!((nx - 0.5).abs() < 1e-5 && (ny - 0.5).abs() < 1e-5, "{nx} {ny}");
    }

    #[test]
    fn hud_light_and_map_compass() {
        let w = FlatWorld;
        let mut inp = inputs(&w);
        let h = HudModel {
            origin: 0x004b_b6cf,
            screen: [740.0, 40.0],
            rotation: [-120.0, 0.0, 0.0],
            scale: 0.003,
            model: 0x9ff,
            model_size: [10, 10, 10],
            depth: 0.0,
            material: [1.0; 4],
        };
        inp.hud_visible = true;
        inp.hud_models = vec![h];
        let f = build_frame(&inp);
        let p = f.pass(PassKind::HudModels).unwrap();
        let u = &f.uniforms[p.draws[0].uniforms];
        let k = normalize([0.0, 0.5, -1.0]);
        assert_eq!(u.light_direction, normalize(k));
        assert_eq!(u.light_back, [0.2, 0.3, 0.4, 1.0]);
        assert_eq!(p.draws[0].draw.light_set, 0);
        assert_eq!(p.draws[0].state.depth.func, CompareFunc::Less);
        assert!(f.pass(PassKind::Minimap).is_some());

        // Map screen: the compass comes from `map_compass`, not the HUD list.
        inp.mode = FrameMode::MapScreen;
        inp.hud_models = vec![];
        inp.map_compass = Some(HudModel { origin: 0x004a_e21b, ..h });
        let f = build_frame(&inp);
        let p = f.pass(PassKind::HudModels).unwrap();
        assert_eq!(p.draws.len(), 1);
        assert_eq!(p.draws[0].origin, 0x004a_e21b);
        assert_eq!(p.clear.unwrap().depth, Some(1.0));
    }
}
