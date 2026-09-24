//! The worker threads the `GameController` constructor (`Cube.exe 0x00459c40`) starts through
//! `startThread 0x00450e70`, and the state they share with the main thread.
//!
//! | Lambda thunk | Worker | Here |
//! |---|---|---|
//! | 0x0046db70 | `chunkMeshThread 0x004690a0` | [`chunk_mesh_loop`] |
//! | 0x0046db80 | `landscapeThread 0x00469590` | [`crate::landscape::LandscapeWorker`] (started by the controller) |
//! | 0x0046dba0 | `zoneThread 0x0046a8a0` | [`zone_loop`] |
//!
//! (The send and receive threads 0x00469c10 / 0x0046b740 are `crate::net`'s.)
//!
//! # Locks
//!
//! The original shares one `GameController` between five threads under three critical
//! sections (plus the landscape section `+0x8005e8`); the port keeps them as named locks in
//! [`ClientShared`]:
//!
//! - **A, `World::lock`** (`0x00601cb0` / `0x00601e90` on the `cube::World` at `GC+0x2e4`):
//!   the client's `cw_world::World` ([`ClientShared::world`], a `RwLock` so the landscape
//!   thread can read while the frame reads) and the creature map with the network lists
//!   (`crate::net::NetShared::world`). Order when both are needed: the `World` first, then the
//!   creature map.
//! - **B, the critical section at `GC+0x8005d0`**: the requested and loaded world, the chunk
//!   window origin, the player's block position and the zone-thread centres
//!   ([`ClientShared::world_cs`], [`WorldCs`]), and the discovered zone/region lists the send
//!   thread reports (`crate::net::NetShared::discovery`, the same section in the original).
//! - **C, the critical section at `GC+0x800600`**: the chunk ring `GC+0x2e0`
//!   ([`ClientShared::mesh_cs`], [`ChunkRing`]). The mesher picks the chunk to build under it
//!   and stores the build under it.
//! - The landscape section `+0x8005e8` guards the `cube::WorldMap` tile cache
//!   ([`ClientShared::tiles`]).
//!
//! Lock order used everywhere: C, then A (`World`), then A (creature map), then B. The mesher
//! is the only thread that holds C while it takes the `World` (the brief `chunk_ready` read);
//! the frame never nests them.
//!
//! The workers take lock A per small unit of work and step aside while the frame thread waits
//! for it ([`FrameGate`]). A unit is a row of columns (a relight step, a mesh pass), a cell of
//! a map tile, a zone insert: well under a millisecond, because the frame takes lock A several
//! times a frame and can wait for a unit (plus a queued one) each time; units of 1..4 ms (a
//! whole relight sweep, a row of map cells) added up to 4..8 ms frame stalls.
//! `CW_CLIENT_STATS=1` logs the frame's lock waits and any worker hold over 8 ms
//! (`CW_CLIENT_STATS_NOTE_MS`); `CW_CLIENT_STATS_WAIT_MS=<ms>` also logs every frame wait over
//! `<ms>` with the worker holds that overlapped it (`crate::profile`).
//!
//! # Differences from the original (Tier C threading, Tier B order)
//!
//! - Zones are generated in a private copy of the world ([`World::generator_copy`], as the
//!   server port's generation thread does), lit (`computeLight`, zone-local) outside the lock,
//!   and inserted under a brief write lock; the original generates in place with no lock and
//!   publishes the zone pointer. The algorithms and their order are unchanged. The unload pass
//!   takes zones and regions out under the write lock and saves them after it ([`unload_pass`]).
//! - `buildChunkMesh` relights (writes) the world, so the mesher takes the `World` write lock
//!   for each row of columns of each relight step and for the props, and the read lock for
//!   each row of columns of its two passes (`cw_render::mesh::build_chunk_mesh_with`); the
//!   original takes no lock. It builds without
//!   lock C (the original builds under it), so the frame never waits for a whole build.
//! - The every-60-seconds character save of the mesher (0x00487520 when `GC+0x388 != 0`) runs
//!   on the main thread (`Controller::update`), which owns the character records.
//!
//! # The client's world
//!
//! It starts as a server world (`world+0xb4 == 0`) with the local player (id 1) in
//! `world+0xb8`; see `singleplayer.rs` for the evidence. [`load_world`] keeps both flags, as
//! `World::load` 0x005a52e0 reinitialises the world in place without touching them.

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use cw_render::map::MapTiles;
use cw_render::mesh::{ChunkBuild, WorldAccess, build_chunk_mesh_with, chunk_ready};
use cw_render::passes::ChunkWindow;
use cw_world::World;

use crate::net::NetShared;
use crate::profile::{self, Phase};

/// `GC+0x2dc`: the chunk window edge, 40 chunks (the constructor's `mov [ebx+0x2dc], 0x28`
/// at 0x00459f39; nothing else writes it).
pub const CHUNK_WINDOW: i32 = 0x28;

/// Chunks per ring axis limit (`< 0x80000`, 0x004691d0 and 0x004890a0).
pub const CHUNK_LIMIT: i32 = 0x80000;

/// The mesher's vertex budget factor: `__ftol2((double)renderDistance * 0.01 *
/// (double)0x1800000)` (0x00469396..0x004693ca: `cvtdq2pd` of `GC+0x180`, `mulsd` by the
/// double 0.01 at 0x00702a48, `fild` of the i64 0x1800000 = 25165824).
pub const VERTEX_BUDGET_UNIT: i64 = 0x0180_0000;

/// The zone thread's initial best squared distance, 0x90000 = 768² blocks
/// (0x0046a9f4).
pub const ZONE_SEARCH_BEST: i32 = 0x90000;

/// The mesher's vertex budget for a render distance (`GC+0x180`, the options' 0..100 slider).
pub fn vertex_budget(render_distance: i32) -> i64 {
    // 0x00469396: `(double)rd * 0.01` (SSE2 mulsd), then `* (double)0x1800000` (x87 after the
    // SSE product is stored and reloaded as double), `__ftol2` truncates.
    let a = f64::from(render_distance) * 0.01f64;
    let b = a * VERTEX_BUDGET_UNIT as f64;
    b as i64
}

// ---------------------------------------------------------------------------------------------
// Lock B: the world request and the zone-thread inputs.

/// The fields under the critical section `GC+0x8005d0` that the zone and mesh threads read.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorldCs {
    /// `GC+0x800a50`, `GC+0x800a54`: the world `startWorld 0x0046f620` asked for.
    pub requested_seed: i32,
    pub requested_name: String,
    /// `GC+0x800448` (`World+0x800164`), `GC+0x378`: the world `World::load 0x005a52e0` last
    /// loaded. `None` before the first load (the original's constructor loads nothing either;
    /// its fields start zero and empty, which is a "match" for a request of `(0, "")`).
    pub loaded: Option<(i32, String)>,
    /// `GC+0x2ac`, `GC+0x2b0`: the chunk window origin (the player's chunk minus half the window,
    /// written by `update` 0x0049cf70).
    pub window_origin: [i32; 2],
    /// `GC+0x2b4`, `GC+0x2b8`: the local player's block position (0x0049cfa8).
    pub player_block: [i32; 2],
    /// `GC+0x2c4`: the zones around which the zone thread generates (the local player's zone;
    /// with the in-process server, which this build never creates, every player's).
    pub centres: Vec<(i32, i32)>,
}

impl WorldCs {
    /// `GC+0x800a50 == GC+0x800448 && name == GC+0x378` (0x004690f0.., 0x0046a8f0..).
    pub fn matches(&self) -> bool {
        match &self.loaded {
            Some((s, n)) => *s == self.requested_seed && *n == self.requested_name,
            None => false,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Lock C: the chunk ring.

/// One `cube::Chunk` record of the ring (0x268 bytes, vtable writers 0x004596e0 / 0x00466bf0).
#[derive(Debug, Clone, Default)]
pub struct ChunkRecord {
    /// `+0x18`: the chunk coordinates the record holds; `None` once released (`0x00486ba0`
    /// frees the buffers; the port also forgets the coordinates so the record is rebuilt when
    /// it is wanted again).
    pub coords: Option<[i32; 2]>,
    /// `+8` (the `cube::ChunkBuffer` list), `+0x20..+0x50` bounds, `+0x23c` vertex count,
    /// `+0x240` props: what `buildChunkMesh 0x0049d910` left.
    pub build: Option<Arc<ChunkBuild>>,
    /// `+0x74`: remesh requested.
    pub dirty: bool,
    /// Bumped on every build or release, for the GPU upload.
    pub version: u64,
}

impl ChunkRecord {
    /// `+0x23c`, 0 without buffers.
    pub fn vertex_count(&self) -> usize {
        self.build.as_ref().map_or(0, |b| b.vertex_count)
    }

    /// `0x00486ba0`: release the buffers.
    pub fn release(&mut self) {
        if self.build.is_some() || self.coords.is_some() {
            self.build = None;
            self.coords = None;
            self.version = self.version.wrapping_add(1);
        }
    }
}

/// `GC+0x2e0`: the ring of `size²` records, index `(y % size) * size + x % size`, with the
/// window `GC+0x2ac`, `+0x2b0`, `+0x2dc`.
#[derive(Debug, Clone)]
pub struct ChunkRing {
    pub window: ChunkWindow,
    pub records: Vec<ChunkRecord>,
}

impl Default for ChunkRing {
    fn default() -> Self {
        ChunkRing::new(CHUNK_WINDOW)
    }
}

impl ChunkRing {
    pub fn new(size: i32) -> ChunkRing {
        ChunkRing { window: ChunkWindow { origin: [0, 0], size }, records: vec![ChunkRecord::default(); (size * size) as usize] }
    }

    /// The ring slot of chunk `(x, y)` (`((y % n) * n + x % n) * 0x268 + GC+0x2e0`).
    pub fn index(&self, x: i32, y: i32) -> usize {
        let n = self.window.size;
        ((y % n) * n + x % n) as usize
    }

    /// Releases every record (0x0046930 world-mismatch branch; the zone thread's
    /// `0x0046aa..` loop over the window).
    pub fn release_all(&mut self) {
        for r in &mut self.records {
            r.release();
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Shared state.

/// What the main thread and the workers share (the parts of `cube::GameController` the worker
/// lambdas reach through their captured `this`).
pub struct ClientShared {
    /// Lock A: the client's world (`GC+0x2e4`).
    pub world: Arc<RwLock<World>>,
    /// Lock A's fairness toward the frame thread ([`FrameGate`]).
    pub gate: Arc<FrameGate>,
    /// Lock A (creature map, network lists) and lock B (discovery lists).
    pub net: Arc<NetShared>,
    /// Lock B: world request and zone-thread inputs.
    pub world_cs: Mutex<WorldCs>,
    /// Lock C: the chunk ring.
    pub mesh_cs: Mutex<ChunkRing>,
    /// The landscape section `+0x8005e8`: the `cube::WorldMap` tile cache (`GC+0x800d44`).
    pub tiles: Arc<Mutex<MapTiles>>,
    /// `GC+0x800584`: the mesh and landscape threads run while it is set.
    pub running: AtomicBool,
    /// `GC+0x8005b0`: the zone thread runs while it is set.
    pub zone_running: AtomicBool,
    /// `GC+0x180` (options `renderDistance`), read by the mesher under lock A.
    pub render_distance: AtomicI32,
    /// The world model table (`world+0x20`) every loaded world shares.
    pub models: Arc<Vec<cw_world::model::Model>>,
    /// The game folder (`Save/` lives under it).
    pub game_dir: PathBuf,
    /// Worlds the zone thread loaded (`World::load` + `0x005fbc90`), for the controller to
    /// finish on the main thread (world list selection, map database).
    pub loaded_events: Mutex<Vec<(i32, String)>>,
    /// The local player's saved positions (`*(player+0x1d28)+0x1c`, a
    /// `std::map<std::pair<int, std::string>, vec3i64>`), which the zone thread reads when it
    /// places the player in a loaded world (0x0044b880 find). The controller keeps it current.
    pub positions: Mutex<std::collections::BTreeMap<crate::persist::WorldKey, [i64; 3]>>,
}

impl ClientShared {
    pub fn new(net: Arc<NetShared>, models: Arc<Vec<cw_world::model::Model>>, game_dir: PathBuf, render_distance: i32) -> ClientShared {
        // `World::World(models, 0)` 0x0058eb00 from the constructor (0x00459f31): a server
        // world, `world+0xb8` the local creature (id 1, `World::setLocalPlayer` 0x00487e90).
        let mut world = World::new(0);
        world.is_client = false;
        world.local_player = Some(LOCAL_PLAYER_ID);
        world.models = Arc::clone(&models);
        ClientShared {
            world: Arc::new(RwLock::new(world)),
            gate: Arc::new(FrameGate::default()),
            net,
            world_cs: Mutex::new(WorldCs::default()),
            mesh_cs: Mutex::new(ChunkRing::default()),
            tiles: Arc::new(Mutex::new(MapTiles::new())),
            running: AtomicBool::new(true),
            zone_running: AtomicBool::new(true),
            render_distance: AtomicI32::new(render_distance),
            models,
            game_dir,
            loaded_events: Mutex::new(Vec::new()),
            positions: Mutex::new(std::collections::BTreeMap::new()),
        }
    }

    // The waits are counted per thread for `CW_CLIENT_STATS` (`crate::profile`).

    pub fn lock_world_cs(&self) -> std::sync::MutexGuard<'_, WorldCs> {
        profile::wait(Phase::WaitCs, || self.world_cs.lock().unwrap_or_else(|e| e.into_inner()))
    }

    pub fn lock_mesh_cs(&self) -> std::sync::MutexGuard<'_, ChunkRing> {
        profile::wait(Phase::WaitRing, || self.mesh_cs.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Lock A for reading. The frame thread goes through [`FrameGate::frame_waits`], a worker
    /// through [`FrameGate::let_frame_in`] first.
    pub fn read_world(&self) -> Held<std::sync::RwLockReadGuard<'_, World>> {
        self.read_world_as("World read lock")
    }

    /// Lock A for writing (see [`ClientShared::read_world`]).
    pub fn write_world(&self) -> Held<std::sync::RwLockWriteGuard<'_, World>> {
        self.write_world_as("World write lock")
    }

    /// [`ClientShared::read_world`] for a worker unit named `what` (the diagnostics' label).
    pub fn read_world_as(&self, what: &'static str) -> Held<std::sync::RwLockReadGuard<'_, World>> {
        let g = self.gate.acquire(|| self.world.read().unwrap_or_else(|e| e.into_inner()));
        Held::new(g, what)
    }

    /// [`ClientShared::write_world`] for a worker unit named `what`.
    pub fn write_world_as(&self, what: &'static str) -> Held<std::sync::RwLockWriteGuard<'_, World>> {
        let g = self.gate.acquire(|| self.world.write().unwrap_or_else(|e| e.into_inner()));
        Held::new(g, what)
    }

    pub fn lock_tiles(&self) -> std::sync::MutexGuard<'_, MapTiles> {
        profile::wait(Phase::WaitTiles, || self.tiles.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// Keeps the workers from starving the frame thread of lock A (Tier C, not in the original,
/// whose threads share the world without locks).
///
/// The workers take lock A once per small unit of work (a row of a chunk build, a relight
/// sweep, a row of a map tile) and take it again at once. `std`'s `RwLock` lets a running
/// thread take a lock that was just released before the waiter it woke gets to run, so the
/// frame thread, which needs the lock three times a frame, could wait through dozens of units
/// (30..60 ms frames while the world loads). The frame thread announces its wait here and the
/// workers hold off their next unit until it has the lock.
#[derive(Debug, Default)]
pub struct FrameGate {
    /// Lock acquisitions the frame thread is waiting for.
    waiting: AtomicI32,
}

thread_local! {
    /// The frame thread is the process's main thread (winit's event loop).
    static IS_FRAME_THREAD: bool = std::thread::current().name() == Some("main");
}

impl FrameGate {
    /// Takes a lock with `lock`: announced on the frame thread, after [`FrameGate::let_frame_in`]
    /// on a worker; the wait counts as [`Phase::WaitWorld`] on the frame thread.
    pub fn acquire<G>(&self, lock: impl FnOnce() -> G) -> G {
        if IS_FRAME_THREAD.with(|m| *m) {
            let t = Instant::now();
            let g = self.frame_waits(|| profile::wait(Phase::WaitWorld, lock));
            profile::attribute_wait(t, Instant::now());
            g
        } else {
            self.let_frame_in();
            lock()
        }
    }

    /// The frame thread waits for a lock inside `f`.
    pub fn frame_waits<T>(&self, f: impl FnOnce() -> T) -> T {
        self.waiting.fetch_add(1, Ordering::SeqCst);
        let r = f();
        self.waiting.fetch_sub(1, Ordering::SeqCst);
        r
    }

    /// A worker, before it takes lock A: while the frame thread waits for it, yield (at most
    /// 50 ms, so a frame thread stuck elsewhere cannot stop the workers).
    pub fn let_frame_in(&self) {
        if self.waiting.load(Ordering::SeqCst) <= 0 {
            return;
        }
        let t = Instant::now();
        while self.waiting.load(Ordering::SeqCst) > 0 && t.elapsed() < Duration::from_millis(50) {
            std::thread::yield_now();
        }
    }
}

/// A lock guard that, with `CW_CLIENT_STATS`, reports a long hold on a worker thread
/// (`crate::profile::worker_note`).
pub struct Held<G> {
    guard: G,
    since: Option<(Instant, &'static str)>,
}

impl<G> Held<G> {
    fn new(guard: G, what: &'static str) -> Held<G> {
        let worker = profile::enabled() && std::thread::current().name().is_some_and(|n| n != "main");
        Held { guard, since: worker.then(|| (Instant::now(), what)) }
    }
}

impl<G: std::ops::Deref> std::ops::Deref for Held<G> {
    type Target = G::Target;
    fn deref(&self) -> &G::Target {
        &self.guard
    }
}

impl<G: std::ops::DerefMut> std::ops::DerefMut for Held<G> {
    fn deref_mut(&mut self) -> &mut G::Target {
        &mut self.guard
    }
}

impl<G> Drop for Held<G> {
    fn drop(&mut self) {
        if let Some((t, what)) = self.since {
            profile::worker_note(what, t.elapsed());
            profile::record_hold(what, t);
        }
    }
}

/// The id the constructor gives the local creature in the creature map (0x00462205:
/// `map[(int64)1]`).
pub const LOCAL_PLAYER_ID: i64 = 1;

/// A fresh client world as `World::load(seed, name)` 0x005a52e0 leaves it: the shared model
/// table, named when `name` is not empty; with a name, `Save/world_<name>.db` opened for the
/// zone, region and time blobs (0x005a56e4). `world+0xb4`/`+0xb8` are the caller's to keep.
pub fn client_world(seed: i32, name: &str, models: &Arc<Vec<cw_world::model::Model>>, game_dir: &std::path::Path) -> World {
    let mut w = World::new(seed);
    w.has_name = !name.is_empty();
    if w.has_name {
        let path = crate::persist::world_db_path(game_dir, name);
        let opened = path.parent().map(std::fs::create_dir_all);
        match (opened, cw_formats::SaveDb::open(&path)) {
            (_, Ok(db)) => {
                if let Ok(Some(blob)) = db.get(cw_world::save::TIME_KEY) {
                    w.load_time_blob(&blob);
                }
                w.attach_save(db);
            }
            (_, Err(e)) => eprintln!("cw-client: {}: {e}", path.display()),
        }
    }
    w.models = Arc::clone(models);
    if !w.has_name {
        // An unnamed world starts at 09:00 (`cw_world::World::time_of_day` doc).
        w.time_of_day = 32_400_000;
    }
    w
}

// ---------------------------------------------------------------------------------------------
// chunkMeshThread 0x004690a0

/// One entry of the mesher's work list (16 bytes: record pointer, squared distance, x, y).
#[derive(Debug, Clone, Copy)]
struct MeshJob {
    slot: usize,
    dist2: u32,
    x: i32,
    y: i32,
}

/// One pass of `chunkMeshThread` (`0x004690a0`, the body of its loop). Returns whether a chunk
/// was built.
///
/// 1. Under lock B: whether the loaded world is the requested one; the window origin.
/// 2. World matches: every ring slot of the window with `0 <= x, y < 0x80000`, keyed by the
///    squared chunk distance from the window centre (`(x - n/2 - ox)² + (y - n/2 - oy)²`),
///    sorted ascending (`std::sort` 0x00455d80, [`cw_math::sort::msvc_sort`]). Under lock A the
///    vertex budget ([`vertex_budget`]). Under lock C, nearest first: past the budget (the running
///    sum of `+0x23c` above it, an i64 compare) the record is released (0x00486ba0); otherwise a
///    record holding other coordinates is released, and a record that is stale or dirty
///    (`+0x74`) is built (`buildChunkMesh 0x0049d910`) when [`chunk_ready`] (0x0046f490) holds,
///    **one build per pass**; its vertex count joins the sum either way.
/// 3. World mismatch: every record is released under lock C.
///
/// The every-60-seconds character save (0x00487520 when `GC+0x388 != 0`) runs on the main
/// thread (`Controller::update`).
pub fn chunk_mesh_pass(shared: &ClientShared) -> bool {
    let (matches, origin) = {
        let cs = shared.lock_world_cs();
        (cs.matches(), cs.window_origin)
    };
    if !matches {
        shared.lock_mesh_cs().release_all();
        return false;
    }
    let mut ring = shared.lock_mesh_cs();
    let n = ring.window.size;
    // The window origin the ring holds is the one of lock B (the original reads `GC+0x2ac`
    // under B at 0x0046910b; the ring has no copy of its own).
    ring.window.origin = origin;
    let mut jobs: Vec<MeshJob> = Vec::new();
    for x in origin[0]..origin[0] + n {
        for y in origin[1]..origin[1] + n {
            if x < 0 || y < 0 || x >= CHUNK_LIMIT || y >= CHUNK_LIMIT {
                continue;
            }
            let dx = x - n / 2 - origin[0];
            let dy = y - n / 2 - origin[1];
            let d = (dx.wrapping_mul(dx)).wrapping_add(dy.wrapping_mul(dy)) as u32;
            jobs.push(MeshJob { slot: ring.index(x, y), dist2: d, x, y });
        }
    }
    // 0x00469370: std::sort by the distance (`+4`), ascending.
    cw_math::sort::msvc_sort(&mut jobs, |a, b| a.dist2 < b.dist2);
    let budget = vertex_budget(shared.render_distance.load(Ordering::Relaxed));
    let mut may_build = true;
    let mut built = false;
    let mut total: i64 = 0;
    let mut i = 0;
    while i < jobs.len() {
        let j = jobs[i];
        i += 1;
        // 0x00469410: `total > budget` (i64, signed high word, unsigned low word).
        if total > budget {
            ring.records[j.slot].release();
            continue;
        }
        let rec = &ring.records[j.slot];
        let stale = rec.coords != Some([j.x, j.y]);
        if stale || rec.dirty {
            if stale {
                ring.records[j.slot].release();
            }
            if may_build {
                let ready = chunk_ready(&shared.read_world_as("mesh ready"), j.x, j.y);
                if ready {
                    may_build = false;
                    // The build runs without lock C (Tier C; the original holds it, and the
                    // frame's fog step and render waited for whole builds). The record takes
                    // its coordinates now (still without buffers, which is what the fog step
                    // and render look at) so remesh marks made during the build land on it;
                    // the dirty flag is taken now so such a mark stays set; the version tells
                    // whether anything released the record meanwhile.
                    let dirty = std::mem::take(&mut ring.records[j.slot].dirty);
                    ring.records[j.slot].coords = Some([j.x, j.y]);
                    let version = ring.records[j.slot].version;
                    drop(ring);
                    let b = build_chunk_mesh_with(&mut SharedWorld(shared), j.x, j.y, dirty);
                    ring = shared.lock_mesh_cs();
                    let rec = &mut ring.records[j.slot];
                    // Released meanwhile (a world switch): the build is dropped; a later pass
                    // picks the chunk again.
                    if rec.version == version {
                        rec.build = b.map(Arc::new);
                        rec.version = rec.version.wrapping_add(1);
                        built = true;
                    }
                }
            }
        }
        total = total.wrapping_add(ring.records[j.slot].vertex_count() as i64);
    }
    built
}

/// [`WorldAccess`] through the shared `World` lock, taken per unit of the build.
struct SharedWorld<'a>(&'a ClientShared);

impl WorldAccess for SharedWorld<'_> {
    fn read(&mut self, f: &mut dyn FnMut(&World)) {
        f(&self.0.read_world_as("mesh read"));
    }
    fn write(&mut self, f: &mut dyn FnMut(&mut World)) {
        f(&mut self.0.write_world_as("mesh write"));
    }
}

/// `chunkMeshThread 0x004690a0`: passes while `GC+0x800584` is set, `Sleep(5)` after each.
pub fn chunk_mesh_loop(shared: Arc<ClientShared>) {
    while shared.running.load(Ordering::Relaxed) {
        chunk_mesh_pass(&shared);
        std::thread::sleep(Duration::from_millis(5));
    }
}

// ---------------------------------------------------------------------------------------------
// zoneThread 0x0046a8a0

/// The zone thread's own state: its copy of the world for generation and the unload timer
/// (`local_e8`, `timeGetTime()` of the last unload pass).
pub struct ZoneThread {
    generator: Option<World>,
    last_unload: Instant,
}

impl Default for ZoneThread {
    fn default() -> Self {
        ZoneThread { generator: None, last_unload: Instant::now() }
    }
}

/// `(v + (v >> 31 & 0x3f)) >> 6`: the region of a zone coordinate (truncating).
fn region_of(z: i32) -> i32 {
    z / 64
}

/// The nearest zone that is not loaded (0x0046a9c6..0x0046ac78): for each centre (list order),
/// the zones `centre ± 3` clamped to `0..=0xffff`, x outer, y inner; the squared block distance
/// from `player_block` to the zone's middle (`zone * 256 + 128`) must be below the best so
/// far (starting at 0x90000, carried across centres, strict `<`). Coordinates past the table
/// (`x * 256 + 128 >= 0x1000080`, `y >= 0x10000`) count as missing, as the original's bounds
/// test falls through to the pick.
pub fn nearest_missing_zone(world: &World, centres: &[(i32, i32)], player_block: [i32; 2]) -> Option<(i32, i32)> {
    let mut best = ZONE_SEARCH_BEST;
    let mut pick = None;
    for &(cx, cy) in centres {
        let (x0, x1) = ((cx - 3).max(0), (cx + 3).min(0xffff));
        let (y0, y1) = ((cy - 3).max(0), (cy + 3).min(0xffff));
        for x in x0..=x1 {
            let mx = x.wrapping_mul(0x100).wrapping_add(0x80);
            for y in y0..=y1 {
                let my = y.wrapping_mul(0x100).wrapping_add(0x80);
                let dx = player_block[0].wrapping_sub(mx);
                let dy = player_block[1].wrapping_sub(my);
                let d = dx.wrapping_mul(dx).wrapping_add(dy.wrapping_mul(dy));
                if d < best {
                    let loaded = x >= 0 && y >= 0 && mx < 0x100_0080 && y < 0x10000 && world.zone(x, y).is_some();
                    if !loaded {
                        best = d;
                        pick = Some((x, y));
                    }
                }
            }
        }
    }
    pick
}

/// A list of zone or region coordinates.
pub type Coords = Vec<(i32, i32)>;

/// The unload pass (0x0046ad8b..0x0046af5d, every second or on a world switch): over the
/// loaded regions (`rx` outer, `ry` inner), every loaded zone no centre is within squared
/// zone distance 16 of goes (`0x005a4890`, unloadZone; with a world switch, every zone);
/// then the region unless a centre's region is within 2 on both axes (`0x005a4800`), then its
/// climate point unless one is within 4 (`0x005a4780`). Returns the zones, regions and points
/// removed, for the generator copy, and their saves.
///
/// The zones and regions are only taken out here, under the world lock; their saves (zlib and
/// one SQLite write per blob, 10..15 ms for a batch) run after it ([`Unloaded::save`], Tier C:
/// the original saves inside `unloadZone`/`unloadRegion`), in the same order.
pub fn unload_pass(world: &mut World, centres: &[(i32, i32)], switching: bool) -> Unloaded {
    let mut out = Unloaded { target: world.save_target(), ..Unloaded::default() };
    for (rx, ry) in world.loaded_regions() {
        for i in 0..64 {
            for j in 0..64 {
                let (zx, zy) = (rx * 64 + i, ry * 64 + j);
                if world.zone(zx, zy).is_none() {
                    continue;
                }
                let near = !switching
                    && centres.iter().any(|&(cx, cy)| {
                        let (dx, dy) = (zx.wrapping_sub(cx), zy.wrapping_sub(cy));
                        dx.wrapping_mul(dx).wrapping_add(dy.wrapping_mul(dy)) < 0x10
                    });
                // `World::unload_zone` without its save.
                if !near
                    && world.region(region_of(zx), region_of(zy)).is_some()
                    && let Some(zone) = world.remove_zone(zx, zy)
                {
                    out.zones.push((zx, zy));
                    out.saves.push(PendingSave::Zone(zone));
                }
            }
        }
        let within = |r: i32| {
            !switching
                && centres
                    .iter()
                    .any(|&(cx, cy)| rx.wrapping_sub(region_of(cx)).wrapping_abs() < r && ry.wrapping_sub(region_of(cy)).wrapping_abs() < r)
        };
        // `World::unload_region` without its save.
        if !within(3)
            && let Some(region) = world.take_region(rx, ry)
        {
            out.regions.push((rx, ry));
            out.saves.push(PendingSave::Region(rx, ry, region));
        }
        if !within(5) && world.remove_region(rx, ry) {
            out.points.push((rx, ry));
        }
    }
    out
}

/// A zone or region the unload pass took out, still to be saved.
enum PendingSave {
    Zone(Box<cw_world::zone::Zone>),
    Region(i32, i32, Box<cw_world::region::Region>),
}

/// What [`unload_pass`] removed.
#[derive(Default)]
pub struct Unloaded {
    pub zones: Coords,
    pub regions: Coords,
    pub points: Coords,
    saves: Vec<PendingSave>,
    target: cw_world::save::SaveTarget,
}

impl Unloaded {
    /// The saves of `World::unloadZone` (`saveZone`) and `World::unloadRegion`
    /// (`saveEntities`), in the pass's order; the write results are not checked, as there.
    pub fn save(self) {
        for p in &self.saves {
            let _ = match p {
                PendingSave::Zone(zone) => self.target.save_zone(zone),
                PendingSave::Region(rx, ry, region) => self.target.save_region_entities(*rx, *ry, region),
            };
        }
    }
}

/// One pass of `zoneThread 0x0046a8a0`. Returns whether a zone was generated.
///
/// 1. Under lock B (0x0046a8f0..0x0046a9b0): whether the loaded world is the requested one,
///    the window origin, the player's block position and the centre list.
/// 2. World matches: the nearest missing zone ([`nearest_missing_zone`]) is generated
///    (`World::generateZone 0x005e4850`, lit by `computeLight` over the zone as generation does
///    at 0x005ede84), its map record marked changed (`0x00602440(x, y)+0x28 = 1`,
///    [`MapTiles::mark_dirty`]), and the zone and its region go on the discovered lists the
///    send thread reports (packets 11 and 12).
///    World mismatch: every chunk of the window is released under lock C.
/// 3. With a mismatch, or more than 1000 ms since the last (`(int)(now - last) >= 0x3e9`): the
///    unload pass ([`unload_pass`]).
/// 4. World mismatch: under lock A, `World::load(seed, name)` (a fresh client world),
///    `0x005fbc90` (the map database, done by the controller from
///    [`ClientShared::loaded_events`]), and the player placed at the position saved for the
///    world (`*(player+0x1d28)+0x1c`, [`ClientShared::positions`]) or else at the world's spawn
///    (`GC+0x8003d4` × 65536, `__ftol2`), copied to the render position, velocity zeroed.
pub fn zone_pass(shared: &ClientShared, st: &mut ZoneThread) -> bool {
    let (matches, centres, player_block, request) = {
        let cs = shared.lock_world_cs();
        (cs.matches(), cs.centres.clone(), cs.player_block, (cs.requested_seed, cs.requested_name.clone()))
    };
    let mut generated = false;
    if matches {
        let pick = nearest_missing_zone(&shared.read_world_as("zone search"), &centres, player_block);
        if let Some((x, y)) = pick {
            generate_one(shared, st, x, y);
            generated = true;
        }
    } else {
        shared.lock_mesh_cs().release_all();
    }
    let now = Instant::now();
    if !matches || now.duration_since(st.last_unload) >= Duration::from_millis(0x3e9) {
        st.last_unload = now;
        // The zones and regions leave under the write lock; they are saved after it.
        let unloaded = unload_pass(&mut shared.write_world_as("zone unload"), &centres, !matches);
        if let Some(g) = &mut st.generator {
            for &(rx, ry) in &unloaded.regions {
                g.forget_region(rx, ry);
            }
            for &(rx, ry) in &unloaded.points {
                g.remove_region(rx, ry);
            }
        }
        unloaded.save();
    }
    if !matches {
        load_world(shared, st, request.0, &request.1);
    }
    generated
}

/// `World::generateZone(x, y)` in the zone thread's copy of the world, then the zone lit and
/// inserted into the client world under lock A.
fn generate_one(shared: &ClientShared, st: &mut ZoneThread, x: i32, y: i32) {
    let g = st.generator.get_or_insert_with(|| shared.read_world_as("zone copy").generator_copy());
    g.pull_play_state(&shared.read_world_as("zone pull"));
    // `generateZone` creates the 3x3 regions around the zone first (0x005186e1), published
    // before the zone so the tick sees them in the original's order of events.
    for dx in -1..=1 {
        for dy in -1..=1 {
            g.create_region(region_of(x) + dx, region_of(y) + dy);
        }
    }
    g.publish_generation(&mut shared.write_world_as("zone publish"));
    // `spawnCellNpc` at the end of `generateZone` reads the cells as the tick has them then
    // (the singleplayer world activates regions while the zone is built).
    g.generate_zone_hooked(x, y, &mut |_, _| {}, &mut |g| g.pull_play_state(&shared.read_world_as("zone pull")));
    let mut zone = g.remove_zone(x, y);
    // 0x005ede84: `computeLight` over the zone with the zone as the column hint. It reads and
    // writes only the zone's own columns (`ZoneColumns`), so it runs here, before the lock:
    // lighting a whole zone takes ~100 ms, which under the write lock stalled the frame (and,
    // through the mesher waiting for the lock with the ring lock held, the fog step).
    if let Some(zone) = zone.as_mut() {
        let (x0, y0) = (x * 256, y * 256);
        cw_render::light::compute_light(&mut cw_render::light::ZoneColumns(zone), x0, y0, x0 + 256, y0 + 256, 0);
    }
    {
        let mut w = shared.write_world_as("zone insert");
        g.publish_generation(&mut w);
        if let Some(zone) = zone
            && w.zone(x, y).is_none()
        {
            w.insert_zone(zone);
        }
        // Creature removals are the server's business: a client world (`world+0xb4`) drops
        // them; the singleplayer world's tick applies them.
        if w.is_client {
            w.removed_creatures.clear();
            w.boss_removals.clear();
        }
    }
    shared.tiles.lock().unwrap_or_else(|e| e.into_inner()).mark_dirty(x, y);
    // The chunks of the zone and of its borders mesh again (their neighbours changed).
    {
        let mut ring = shared.lock_mesh_cs();
        for r in ring.records.iter_mut() {
            if let Some([cx, cy]) = r.coords {
                let (bx, by) = (cx * 32, cy * 32);
                if bx + 32 >= x * 256 && bx <= x * 256 + 256 && by + 32 >= y * 256 && by <= y * 256 + 256 {
                    r.dirty = true;
                }
            }
        }
    }
    let mut d = shared.net.discovery.lock().unwrap_or_else(|e| e.into_inner());
    d.zones.push((x, y));
    let r = (region_of(x), region_of(y));
    if !d.regions.contains(&r) {
        d.regions.push(r);
    }
}

/// The world switch of the zone thread (0x0046af68..0x0046b6a0).
fn load_world(shared: &ClientShared, st: &mut ZoneThread, seed: i32, name: &str) {
    let world = client_world(seed, name, &shared.models, &shared.game_dir);
    let spawn = [world.spawn[0], world.spawn[1], 0.0f32];
    let saved = shared.positions.lock().unwrap_or_else(|e| e.into_inner()).get(&(seed, name.as_bytes().to_vec())).copied();
    st.generator = Some(world.generator_copy());
    {
        let mut w = shared.write_world();
        // `World::load` reinitialises in place: `world+0xb4`/`+0xb8` stay. The old world's
        // zones and regions were saved by the unload pass of the switch. The switch runs on
        // the zone thread (0x0046afab), so its `srand(seed)` (0x005a532a) reseeds that
        // thread's CRT stream, not the main thread's: the tick's `rng` carries on.
        let (local, is_client, rng) = (w.local_player, w.is_client, w.rng);
        *w = world;
        w.local_player = local;
        w.is_client = is_client;
        w.rng = rng;
        let mut nw = shared.net.world.lock().unwrap_or_else(|e| e.into_inner());
        let id = nw.local_player;
        if let Some(e) = nw.entities.get_mut(&id) {
            // 0x0044b880: the position saved for `(seed, name)`, else `__ftol2(spawn *
            // 65536.0f)` per axis (0x0046b5a0..).
            let mut pos = [0i64; 3];
            for i in 0..3 {
                pos[i] = saved.map_or((spawn[i] * 65536.0f32) as i64, |p| p[i]);
                e.0[i * 8..i * 8 + 8].copy_from_slice(&pos[i].to_le_bytes());
            }
            // creature+0x34..+0x3f: the velocity (entity +0x24).
            e.0[0x24..0x30].fill(0);
            if let Some(s) = nw.states.get_mut(&id) {
                s.riding.render_pos = pos;
            }
        }
    }
    shared.lock_world_cs().loaded = Some((seed, name.to_string()));
    shared.loaded_events.lock().unwrap_or_else(|e| e.into_inner()).push((seed, name.to_string()));
    let mut d = shared.net.discovery.lock().unwrap_or_else(|e| e.into_inner());
    d.zones.clear();
    d.regions.clear();
}

/// `zoneThread 0x0046a8a0`: passes while `GC+0x8005b0` is set, `Sleep(20)` after each.
pub fn zone_loop(shared: Arc<ClientShared>) {
    let mut st = ZoneThread::default();
    while shared.zone_running.load(Ordering::Relaxed) {
        zone_pass(&shared, &mut st);
        std::thread::sleep(Duration::from_millis(0x14));
    }
}

/// The two worker threads of this module, joined on drop (`GameController::~GameController`
/// 0x00466d90 clears the flags and waits).
pub struct Workers {
    shared: Arc<ClientShared>,
    handles: Vec<std::thread::JoinHandle<()>>,
}

impl Workers {
    /// Starts the mesh thread and the zone thread, in the constructor's order.
    pub fn start(shared: &Arc<ClientShared>) -> Workers {
        let mut handles = Vec::new();
        let s = Arc::clone(shared);
        handles.push(std::thread::Builder::new().name("chunkMesh".into()).spawn(move || chunk_mesh_loop(s)).expect("mesh thread"));
        let s = Arc::clone(shared);
        handles.push(std::thread::Builder::new().name("zone".into()).spawn(move || zone_loop(s)).expect("zone thread"));
        Workers { shared: Arc::clone(shared), handles }
    }

    pub fn stop(&mut self) {
        self.shared.running.store(false, Ordering::Relaxed);
        self.shared.zone_running.store(false, Ordering::Relaxed);
        for h in self.handles.drain(..) {
            let _ = h.join();
        }
    }
}

impl Drop for Workers {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_matches_the_x87_product() {
        assert_eq!(vertex_budget(0), 0);
        assert_eq!(vertex_budget(100), 0x0180_0000);
        // 0.01 is not exact in binary: 50 * 0.01 = 0.5 exactly after rounding.
        assert_eq!(vertex_budget(50), 0x00c0_0000);
    }

    #[test]
    fn ring_indexing_wraps() {
        let r = ChunkRing::new(40);
        assert_eq!(r.index(0, 0), 0);
        assert_eq!(r.index(41, 0), 1);
        assert_eq!(r.index(0, 41), 40);
    }

    #[test]
    fn nearest_missing_prefers_the_player_zone() {
        let w = World::new(1);
        let pick = nearest_missing_zone(&w, &[(100, 200)], [100 * 256 + 10, 200 * 256 + 20]);
        assert_eq!(pick, Some((100, 200)));
        // Nothing within 768 blocks of the player (the zone middles of 97..=103 are all farther
        // than 0x90000 squared blocks): no pick. (Far larger distances wrap the i32 square, as
        // the original's `imul` does.)
        let pick = nearest_missing_zone(&w, &[(100, 200)], [104 * 256 + 128 + 700, 200 * 256 + 128]);
        assert_eq!(pick, None);
    }

    #[test]
    fn world_request_matching() {
        let mut cs = WorldCs::default();
        assert!(!cs.matches());
        cs.loaded = Some((0, String::new()));
        assert!(cs.matches());
        cs.requested_seed = 5;
        assert!(!cs.matches());
    }

    /// The unload pass takes zones and regions out under the world lock and leaves their saves
    /// (zlib and SQLite, 10..15 ms for a batch) for after it.
    #[test]
    fn unload_pass_saves_after_the_lock() {
        let mut w = World::new(1);
        w.has_name = true;
        w.attach_save(cw_formats::SaveDb::in_memory().unwrap());
        w.create_region(1, 3);
        let mut z = cw_world::zone::Zone::new(100, 200);
        z.dirty = true;
        w.insert_zone(Box::new(z));
        let key = cw_world::save::zone_key(100, 200);
        let unloaded = unload_pass(&mut w, &[], true);
        assert_eq!(unloaded.zones, vec![(100, 200)]);
        assert_eq!(unloaded.regions, vec![(1, 3)]);
        assert!(w.zone(100, 200).is_none() && w.region(1, 3).is_none());
        assert!(w.saved_blob(&key).is_none(), "saved under the lock");
        unloaded.save();
        assert!(w.saved_blob(&key).is_some());
    }

    #[test]
    fn mesh_pass_releases_everything_on_a_mismatch() {
        let net = NetShared::new();
        let shared = ClientShared::new(net, Arc::new(Vec::new()), PathBuf::new(), 100);
        {
            let mut ring = shared.lock_mesh_cs();
            ring.records[0].coords = Some([0, 0]);
            ring.records[0].build = Some(Arc::new(ChunkBuild::default()));
        }
        assert!(!chunk_mesh_pass(&shared));
        assert!(shared.lock_mesh_cs().records[0].build.is_none());
    }
}
