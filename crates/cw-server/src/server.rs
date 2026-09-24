//! Shared server state and the main tick (`main` 0x00549c50, note 3.1), with the parts of
//! `World::tick` (0x005322d0) ported so far: the clock, the per-player region activation and
//! mission discovery, and the creatures made from the spawns of the zones around players.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::net::TcpStream;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cw_math::MsvcRand;
use cw_net::EntityData;
use cw_net::ServerUpdate;
use cw_net::packet::{BlockAction, Hit, Interact, Mission, Passive, Shoot};
use cw_world::World;
use cw_world::missions::{MissionRecord, is_equipment};
use cw_world::save::ModifiedBlock;
use cw_world::world::PlayerInfo;

use cw_sim::combat::{CreatureState, apply_hit, apply_passives};
use cw_sim::physics::move_creature;
use cw_sim::update::update_creature;
use cw_sim::creature::entity_from_spawn;
use cw_sim::interact::{K, dist_sq, ground_item_record, handle_interacts, static_record, statics_pass, tail};

/// Entity ids handed to players (`acceptLoop`: the first free of 1..10).
pub const MAX_PLAYERS: i64 = 10;

/// What a connection's send thread drains each frame (`cube::Connection` +0x18, +0x80,
/// +0x88, +0x90).
#[derive(Default)]
pub struct Pending {
    pub update: ServerUpdate,
    pub chat: VecDeque<(i64, Vec<u16>)>,
    pub zones: Vec<(i32, i32)>,
    pub regions: Vec<(i32, i32)>,
}

/// One client (`cube::Connection`, 0xb0 bytes).
pub struct Conn {
    pub id: i64,
    pub stream: Mutex<TcpStream>,
    pub pending: Mutex<Pending>,
    pub alive: AtomicBool,
    /// Entity updates received, for `--verbose`.
    pub updates_seen: std::sync::atomic::AtomicU64,
}

pub struct Server {
    pub world: Mutex<World>,
    /// The world creature map (`world+4`): the players' entity data by id, and the creatures
    /// the tick made from zone spawns.
    pub entities: Mutex<BTreeMap<i64, EntityData>>,
    /// `server+0x28`: connection slots.
    pub slots: Mutex<Vec<Option<Arc<Conn>>>>,
    /// The accept thread's `rand` stream (never seeded by the original).
    pub accept_rng: Mutex<MsvcRand>,
    /// The main thread's `rand` stream as `World::load` leaves it (`srand(seed)` and the 77
    /// draws of the sub-seed table); every `rand` of the world tick comes from it.
    pub tick_rng: Mutex<MsvcRand>,
    /// What the original keeps on `cube::Creature` outside the entity block (threat, buffs,
    /// inventory, pet), by creature id.
    pub states: Mutex<BTreeMap<i64, CreatureState>>,
    /// `main` step 5: the zones players stand in, for the generation thread.
    pub player_zones: Mutex<Vec<(i32, i32)>>,
    /// Client hits, passives and shoots waiting for the next tick (`server+0x34/0x3c/0x44`).
    pub inbox: Mutex<(Vec<Hit>, Vec<Passive>, Vec<Shoot>)>,
    /// Client Interact packets (id 6) by player, queued on the creature in the original
    /// (`creature+0x130c`) for the tick.
    pub interacts: Mutex<Vec<(i64, Interact)>>,
    /// `world+0x14`: the live projectiles the creatures' skills fire (client shoots are relayed,
    /// not simulated: `takeShoots` 0x004281d0).
    pub projectiles: Mutex<Vec<cw_sim::projectile::Projectile>>,
    /// `world+0x8000bc`: the tick's millisecond counter (`cw_sim::day`).
    pub clock: Mutex<cw_sim::day::Clock>,
    pub seed: i32,
    pub running: AtomicBool,
    /// `--verbose`: log client packets and creature spawns.
    pub verbose: bool,
}

impl Server {
    pub fn new(seed: i32, game_dir: &Path, with_save: bool) -> Result<Server, String> {
        let mut world = World::new(seed);
        let mut models = Vec::new();
        cw_world::model::load_models_from_game_dir(game_dir, &mut models).map_err(|e| format!("model table: {e}"))?;
        world.models = Arc::new(models);
        if with_save {
            let dir = game_dir.join("Save");
            std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            let path = dir.join(format!("world_server_{seed}.db"));
            let db = cw_formats::SaveDb::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            if let Ok(Some(blob)) = db.get(cw_world::save::TIME_KEY) {
                world.load_time_blob(&blob);
            }
            world.attach_save(db);
        }
        let (_, tick_rng) = cw_world::seeds::Seeds::from_seed_with_rng(seed);
        Ok(Server {
            world: Mutex::new(world),
            entities: Mutex::new(BTreeMap::new()),
            slots: Mutex::new(Vec::new()),
            accept_rng: Mutex::new(MsvcRand::new(1)),
            tick_rng: Mutex::new(tick_rng),
            states: Mutex::new(BTreeMap::new()),
            player_zones: Mutex::new(Vec::new()),
            inbox: Mutex::new((Vec::new(), Vec::new(), Vec::new())),
            projectiles: Mutex::new(Vec::new()),
            interacts: Mutex::new(Vec::new()),
            clock: Mutex::new(cw_sim::day::Clock::default()),
            seed,
            running: AtomicBool::new(true),
            verbose: false,
        })
    }

    pub fn connections(&self) -> Vec<Arc<Conn>> {
        self.slots.lock().unwrap().iter().flatten().cloned().collect()
    }

    /// Queues a chat line on every connection, the sender's included (case 10 of receiveLoop).
    pub fn broadcast_chat(&self, sender: i64, text: Vec<u16>) {
        for c in self.connections() {
            c.pending.lock().unwrap().chat.push_back((sender, text.clone()));
        }
    }
}

fn i32_at(b: &[u8], o: usize) -> i32 {
    i32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

fn i64_at(b: &[u8], o: usize) -> i64 {
    i64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

/// `(i32)(pos / 65536) / 256`: the zone coordinate of a 16.16 position, both divisions
/// truncating (`__alldiv`, then `cdq; and edx, 0xff; add; sar 8`).
fn zone_of(pos: i64) -> i32 {
    ((pos / 65536) as i32) / 256
}

/// `(v + (v >> 31 & 7)) >> 3`: the cell of a zone coordinate, truncating.
fn div8(v: i32) -> i32 {
    v / 8
}

/// The players of the creature map: hostile type 0.
fn players_of(entities: &BTreeMap<i64, EntityData>) -> Vec<PlayerInfo> {
    entities
        .iter()
        .filter(|(_, e)| e.0[0x50] == 0)
        .map(|(id, e)| PlayerInfo { id: *id, level: i32_at(&e.0, 0x180), pos: [i64_at(&e.0, 0), i64_at(&e.0, 8), i64_at(&e.0, 16)] })
        .collect()
}

fn to_packet(m: &MissionRecord) -> Mission {
    Mission { cell_x: m.cell_x, cell_y: m.cell_y, words: m.words, b40: m.b40, b41: m.b41, tail: m.tail }
}

/// `main`'s loop: advance the clock, run the world tick, relay client actions, serve discovery
/// requests, refresh the player zone list, sleep 20 ms.
pub fn tick_loop(server: Arc<Server>) {
    let mut last = Instant::now();
    while server.running.load(Ordering::Relaxed) {
        let now = Instant::now();
        let dt = now.duration_since(last).as_millis() as i64;
        last = now;

        // Client actions gathered since the last tick (takeHits, takePassives, takeShoots).
        let (hits, passives, shoots) = std::mem::take(&mut *server.inbox.lock().unwrap());
        let interacts = std::mem::take(&mut *server.interacts.lock().unwrap());
        let conns = server.connections();
        let mut missions: Vec<MissionRecord> = Vec::new();
        let mut out = ServerUpdate::default();
        {
            let mut world = server.world.lock().unwrap();
            {
                let mut entities = server.entities.lock().unwrap();
                let mut states = server.states.lock().unwrap();
                world_tick(&server, &mut world, &mut entities, &mut states, dt as i32, interacts, hits, passives, &mut missions, &mut out);
            }
            // broadcastUpdates 0x004272d0: hits, shoots, passives, the tick's lists and its
            // missions go to everyone; sounds only to clients within 200 blocks.
            let positions: BTreeMap<i64, [i64; 3]> = server.entities.lock().unwrap().iter().map(|(id, e)| (*id, [i64_at(&e.0, 0), i64_at(&e.0, 8), i64_at(&e.0, 16)])).collect();
            for c in &conns {
                let mut p = c.pending.lock().unwrap();
                p.update.hits.extend(out.hits.iter().cloned());
                p.update.block_actions.extend(out.block_actions.iter().cloned());
                // The tick's own shoots (`out+0x20`: the creatures' projectiles), then the
                // clients' ones that `takeShoots` 0x004281d0 appended (`list::insert(end(), ..)`
                // through 0x00421710).
                p.update.shoots.extend(out.shoots.iter().cloned());
                p.update.shoots.extend(shoots.iter().cloned());
                p.update.passives.extend(out.passives.iter().cloned());
                p.update.kills.extend(out.kills.iter().cloned());
                p.update.damage.extend(out.damage.iter().cloned());
                p.update.statics.extend(out.statics.iter().cloned());
                p.update.zone_items.extend(out.zone_items.iter().cloned());
                p.update.items_8.extend(out.items_8.iter().cloned());
                p.update.pickups.extend(out.pickups.iter().cloned());
                // The region activations and discoveries of the player pass, then the records
                // the creature loop pushed (a mission advanced or completed by a kill,
                // `applyHit` 0x004cea80).
                p.update.missions.extend(missions.iter().map(to_packet));
                p.update.missions.extend(out.missions.iter().cloned());
                if let Some(me) = positions.get(&c.id) {
                    for pt in &out.particles {
                        let d = (0..3).map(|i| (i64::from_le_bytes(pt.0[i * 8..i * 8 + 8].try_into().unwrap()).wrapping_sub(me[i])) as f32 * K).collect::<Vec<f32>>();
                        if (d[1] * d[1] + d[0] * d[0]) + d[2] * d[2] < 40000.0f32 {
                            p.update.particles.push(pt.clone());
                        }
                    }
                    for s in &out.sounds {
                        let d = (0..3).map(|i| f32::from_le_bytes(s.0[i * 4..i * 4 + 4].try_into().unwrap()) - me[i] as f32 * K).collect::<Vec<f32>>();
                        if (d[1] * d[1] + d[0] * d[0]) + d[2] * d[2] < 40000.0f32 {
                            p.update.sounds.push(s.clone());
                        }
                    }
                }
            }
            // Discovery requests (note 2.8).
            for c in &conns {
                let (zones, regions) = {
                    let mut p = c.pending.lock().unwrap();
                    (std::mem::take(&mut p.zones), std::mem::take(&mut p.regions))
                };
                let mut add = ServerUpdate::default();
                for (zx, zy) in zones {
                    zone_discovered(&world, zx, zy, &mut add);
                }
                for (rx, ry) in regions {
                    region_discovered(&world, rx, ry, &mut add);
                }
                if !add.is_empty() {
                    let mut p = c.pending.lock().unwrap();
                    p.update.block_actions.extend(add.block_actions);
                    p.update.zone_items.extend(add.zone_items);
                    p.update.statics.extend(add.statics);
                    p.update.missions.extend(add.missions);
                }
            }
        }
        // Step 5: the zone of every player (hostile type 0).
        let zones: Vec<(i32, i32)> = server.entities.lock().unwrap().values().filter(|e| e.0[0x50] == 0).map(|e| (zone_of(i64_at(&e.0, 0)), zone_of(i64_at(&e.0, 8)))).collect();
        *server.player_zones.lock().unwrap() = zones;
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The state of `World::tick` that the port keeps outside the `World`: the clock
/// (`world+0x8000bc`, `cw_sim::day`), the live projectiles (`world+0x14`) and the logging
/// switches. The server keeps it in [`Server`]; the client's singleplayer world keeps its own
/// (`cw-client` `singleplayer.rs`).
pub struct TickCtx<'a> {
    pub clock: &'a mut cw_sim::day::Clock,
    pub projectiles: &'a mut Vec<cw_sim::projectile::Projectile>,
    /// `--verbose`: log spawns and the combat details.
    pub verbose: bool,
    /// Print the "Missions of region .. activated." line (the dedicated server's log).
    pub log_regions: bool,
    /// Where the tick puts its save-database writes (the `time` blob) instead of doing them:
    /// the client ticks on its frame thread and writes them on a worker. `None` writes them
    /// in place, as the original does.
    pub deferred_saves: Option<&'a mut Vec<cw_world::save::DeferredPut>>,
}

/// What the tick calls per creature after its update and move (`cw-client` advances the walk
/// cycle there, 0x0061e9c6..).
pub type CreatureHook<'a> = dyn FnMut(&World, &BTreeMap<i64, EntityData>, &BTreeMap<i64, CreatureState>, i64) + 'a;

/// The ported slices of `World::tick`, under the world lock: the player loop
/// (0x00532809..0x00532aa5) and the creature spawns of the active zones
/// (0x00535702..0x00536253). The server's `rand()` stream is swapped into `world.rng` for the
/// whole tick (every `rand()` of the tick draws from it; nothing between the original's swaps
/// used the served world's own stream).
#[allow(clippy::too_many_arguments)]
pub fn world_tick(server: &Server, world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, dt: i32, interacts: Vec<(i64, Interact)>, hits: Vec<Hit>, passives: Vec<Passive>, missions: &mut Vec<MissionRecord>, out: &mut ServerUpdate) {
    let mut clock = server.clock.lock().unwrap();
    let mut projectiles = server.projectiles.lock().unwrap();
    let mut rng = server.tick_rng.lock().unwrap();
    std::mem::swap(&mut world.rng, &mut rng);
    let mut ctx = TickCtx { clock: &mut clock, projectiles: &mut projectiles, verbose: server.verbose, log_regions: true, deferred_saves: None };
    world_tick_with(&mut ctx, world, entities, states, dt, interacts, hits, passives, missions, out, &mut |_, _, _, _| {});
    std::mem::swap(&mut world.rng, &mut rng);
}

/// [`world_tick`] on any world: `world.rng` is the tick's `rand()` stream, `ctx` the state
/// outside the world. The passes the original guards with `world+0xb4` (the client's world)
/// are skipped when [`World::is_client`] is set: the region activation and mission discovery of
/// the player loop (0x005328db) and the zone spawns (0x00535708); the statics, creature and
/// interaction passes carry their own guards (`cw_sim`). `hook` runs after each creature's
/// update and move.
#[allow(clippy::too_many_arguments)]
pub fn world_tick_with(ctx: &mut TickCtx<'_>, world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, dt: i32, interacts: Vec<(i64, Interact)>, hits: Vec<Hit>, passives: Vec<Passive>, missions: &mut Vec<MissionRecord>, out: &mut ServerUpdate, hook: &mut CreatureHook<'_>) {
    // The clock (0x00532340..0x005325fd, `cw_sim::day`): ten times real time (a hundred while
    // a player lies in bed), 09:00 in an unnamed world, the day rollover re-arming every
    // region's missions, and the `time` blob every ten seconds of ticks.
    let play_ms = {
        if cw_sim::day::advance_clock(world, ctx.clock, entities, dt)
            && let Some(put) = world.time_save()
        {
            match ctx.deferred_saves.as_deref_mut() {
                Some(sink) => sink.push(put),
                None => {
                    let _ = put.write();
                }
            }
        }
        ctx.clock.play_ms
    };
    world.players = players_of(entities);
    let players = world.players.clone();
    // The original keeps the zones around players in a `std::set<Zone*>`, ordered by address;
    // coordinate order stands in for it.
    let mut active: BTreeSet<(i32, i32)> = BTreeSet::new();
    for p in &players {
        let (zx, zy) = (zone_of(p.pos[0]), zone_of(p.pos[1]));
        for x in zx - 1..=zx + 1 {
            for y in zy - 1..=zy + 1 {
                if world.zone(x, y).is_some() {
                    active.insert((x, y));
                }
            }
        }
        // 0x005328db: the rest of the player loop is the server's.
        if world.is_client {
            continue;
        }
        // Region activation: the region of the climate point nearest to the player, once.
        let (bx, by) = ((p.pos[0] / 65536) as i32, (p.pos[1] / 65536) as i32);
        let (rx, ry) = world.nearest_climate_region(bx, by);
        if world.region(rx, ry).is_some_and(|r| !r.missions_active) {
            world.activate_region_missions(rx, ry, missions);
            if ctx.log_regions {
                println!("Missions of region {rx},{ry} activated.");
            }
        }
        // Mission discovery in the player's cell.
        if let Some(rec) = world.discover_mission(div8(zx), div8(zy), p.pos[0], p.pos[1]) {
            missions.push(rec);
        }
    }
    apply_removals(world, entities);
    // The clients' hits (`applyHit`, 0x00532b40) and passives (0x00532be0), then the statics
    // pass (0x00533129), the spawns (0x00535702), then per creature in id order its
    // interactions (0x005362e2), buffs and the HP clamp, and the tail.
    let mut dirty: BTreeSet<(i32, i32)> = BTreeSet::new();
    for h in &hits {
        apply_hit(world, entities, states, h, out, &mut dirty, ctx.verbose);
    }
    apply_passives(entities, states, &passives);
    statics_pass(world, entities, states, &active, dt, play_ms, out, &mut dirty, ctx.verbose);
    // 0x00533b5c..0x00535702: spawn schedules and countdowns, the despawn, the players' pets,
    // orphaned pets and the dead creatures (`cw_sim::statics`).
    cw_sim::statics::creature_pass(world, entities, states, &active, dt, out, &mut dirty, ctx.verbose);
    // 0x00535708: the spawns are the server's.
    if world.has_name && !world.is_client {
        for &(zx, zy) in &active {
            spawn_creatures_with(world, entities, states, zx, zy, ctx.verbose);
        }
    }
    let mut by_player: BTreeMap<i64, Vec<Interact>> = BTreeMap::new();
    for (id, it) in interacts {
        by_player.entry(id).or_default().push(it);
    }
    // The per-creature loop (0x005362e2, id order): a player's queued interactions, then
    // the HP clamp, timers, buffs, regeneration and threat of `update_creature`.
    let ids: Vec<i64> = entities.keys().copied().collect();
    for id in ids {
        // 0x005362ac: the creature's interaction queue (`creature+0x130c`) is dispatched for
        // every creature: a player's client packets, a villager's RandomInteraction uses.
        let mut packets: Vec<Interact> = by_player.get(&id).cloned().unwrap_or_default();
        if let Some(root) = states.get_mut(&id).and_then(|st| st.ai_root.as_mut()) {
            cw_sim::behaviors_more::take_queued_interactions(root, &mut packets);
        }
        if !packets.is_empty() {
            handle_interacts(world, entities, states, id, &packets, out, &mut dirty, ctx.verbose);
        }
        if !entities.contains_key(&id) {
            continue;
        }
        if update_creature(world, entities, states, ctx.projectiles, &mut dirty, id, dt, out) {
            move_creature(world, entities, states, id, dt, out);
        }
        hook(world, entities, states, id);
        // Pets the mode machine summoned (0x0053b568) join the world's creature set.
        if let Some(st) = states.get_mut(&id) {
            for (pet, _) in st.modes.summoned.drain(..) {
                world.creature_ids.insert(pet);
            }
            st.modes.tamed.clear();
        }
        if ctx.verbose && std::env::var("CW_TRACE_ID").ok().and_then(|v| v.parse::<i64>().ok()) == Some(id)
            && let Some(e) = entities.get(&id)
        {
            let p: Vec<f64> = (0..3).map(|i| i64::from_le_bytes(e.0[i * 8..i * 8 + 8].try_into().unwrap()) as f64 / 65536.0).collect();
            let v: Vec<f32> = (0..3).map(|i| f32::from_le_bytes(e.0[0x24 + i * 4..0x28 + i * 4].try_into().unwrap())).collect();
            println!("trace {id}: dt {dt} pos {:.3},{:.3},{:.3} vel {:?} phys {:#x} flags {:#x} hp {} mode {:#x} mt {} hc {} mp {} hostile {} parent {}", p[0], p[1], p[2], v, u32::from_le_bytes(e.0[0x4c..0x50].try_into().unwrap()), u16::from_le_bytes(e.0[0x114..0x116].try_into().unwrap()), f32::from_le_bytes(e.0[0x15c..0x160].try_into().unwrap()), e.0[0x58], i32::from_le_bytes(e.0[0x5c..0x60].try_into().unwrap()), i32::from_le_bytes(e.0[0x60..0x64].try_into().unwrap()), f32::from_le_bytes(e.0[0x160..0x164].try_into().unwrap()), e.0[0x50], i64::from_le_bytes(e.0[0x188..0x190].try_into().unwrap()));
        }
    }
    // 0x00546830: the projectiles, after the creatures (and the airships, not ported).
    cw_sim::projectile::update_projectiles(world, entities, states, ctx.projectiles, dt, out, &mut dirty);
    tail(world, &dirty, out);
    // 0x0054710b..0x005471d6: the tick's block actions (terrain destruction) go into their
    // zones' modified-block vectors (`zone+0x68`), which `saveZone` writes.
    world.push_block_writes(out.block_actions.iter().map(|b| ModifiedBlock { x: b.x, y: b.y, z: b.z, block: b.block, time: b.time }));
    // A creature gone by any path (daily removal, mission restart, disconnect) takes its
    // state with it.
    states.retain(|id, _| entities.contains_key(id));
}

/// The creatures the world deleted: spawns `spawnCellNpc` dropped, and the bosses (appearance
/// flag 0x2000, home zone in the cell) of cells whose mission restarted.
pub fn apply_removals(world: &mut World, entities: &mut BTreeMap<i64, EntityData>) {
    for id in world.removed_creatures.drain(..) {
        entities.remove(&id);
    }
    for (cx, cy) in std::mem::take(&mut world.boss_removals) {
        let ids: Vec<i64> = entities
            .iter()
            .filter(|(_, e)| u16_at(&e.0, 0x6e) & 0x2000 != 0 && div8(i32_at(&e.0, 0x1a0)) == cx && div8(i32_at(&e.0, 0x1a4)) == cy)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            entities.remove(&id);
            world.creature_ids.remove(&id);
        }
    }
}

/// `World::tick` 0x00535908..0x005361ed: every spawn of a zone near a player becomes a creature
/// when its time window allows, no creature of its id exists, it was not killed today, and a
/// player is within its radius. A settlement spawn in a cell running a type-5 mission is
/// re-rolled first with `srand(mission seed + spawn id)`.
pub fn spawn_creatures(server: &Server, world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, zx: i32, zy: i32) {
    let mut rng = server.tick_rng.lock().unwrap();
    std::mem::swap(&mut world.rng, &mut rng);
    spawn_creatures_with(world, entities, states, zx, zy, server.verbose);
    std::mem::swap(&mut world.rng, &mut rng);
}

/// [`spawn_creatures`] with `world.rng` as the tick's `rand()` stream.
pub fn spawn_creatures_with(world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, zx: i32, zy: i32, verbose: bool) {
    let cell = world.cell(div8(zx), div8(zy)).copied();
    let n = world.zone(zx, zy).map_or(0, |z| z.spawns.len());
    let (day, time) = (world.day, world.time_of_day);
    for k in 0..n {
        let mut s = {
            let Some(z) = world.zone(zx, zy) else { return };
            let Some(s) = z.spawns.get(k) else { return };
            s.clone()
        };
        // A time window (+0x40..+0x44) the time of day must fall in.
        if s.f38[2] != s.f38[3] && (time < s.f38[2] as i32 || time > s.f38[3] as i32) {
            continue;
        }
        let id = s.id();
        if entities.contains_key(&id) {
            continue;
        }
        // A spawn killed today (+0x38 counter, +0x3c day) does not come back before tomorrow.
        if (s.f38[0] as i32) < 0 {
            s.f38[0] = 0;
            if let Some(z) = world.zone_mut(zx, zy) {
                z.spawns[k].f38[0] = 0;
            }
        }
        if s.f38[0] != 0 && s.f38[1] as i32 == day {
            continue;
        }
        // A player within the spawn's radius.
        if !world.players.iter().any(|p| s.radius * s.radius > dist_sq(p.pos, [s.x, s.y, s.z])) {
            continue;
        }
        if let Some(c) = cell
            && c.mission.f34 == 5
            && s.appearance.flags & 0x1000 != 0
        {
            if s.f28 != 6 {
                s.level = c.mission.f3c;
                s.b58 = c.mission.b40;
            }
            world.rng.seed((c.mission.f30 as u32).wrapping_add(id as u32));
            // initClassAndEquipment(spawn, 1) 0x004fb480: the flag skips the per-type combat
            // floats, skill lists and the 0x200 boost (0x004fbb85).
            world.init_creature_flag(&mut s, true);
            if s.appearance.flags & 0x200 != 0 {
                if s.b10e8 != 0 {
                    s.appearance.flags |= 0x2000;
                }
            } else {
                for slot in [7usize, 6, 5, 2, 4, 3, 1, 8, 9] {
                    let mut it = s.equipment[slot];
                    world.adjust_item_level(&mut it, 0.05f32, false);
                    s.equipment[slot] = it;
                }
            }
            for page in &mut s.inventory.pages {
                for slot in page.iter_mut() {
                    if slot.count != 0 && is_equipment(slot.item.item_type, slot.item.sub_type) {
                        slot.item.level = c.mission.f3c as u16;
                        if matches!(slot.item.item_type, 3..=9) {
                            let mut r = c.mission.b40.wrapping_add(1);
                            if world.rng.rand() % 20 == 0 {
                                r = r.wrapping_add(1);
                            }
                            if world.rng.rand() % 100 == 0 {
                                r = r.wrapping_add(1);
                            }
                            slot.item.rarity = r.min(4);
                        }
                    }
                }
            }
            if let Some(z) = world.zone_mut(zx, zy) {
                z.spawns[k] = s.clone();
            }
        }
        let e = entity_from_spawn(&s, (zx, zy), k as i32);
        if verbose {
            println!("Spawned creature {id} type {} level {} in zone {zx},{zy}", s.entity_type, s.level);
        }
        let mut cs = CreatureState::from_spawn(&s);
        // 0x00535f1b: the spawn radius (`spawn+0x08` into `creature+0x1d2c`).
        cs.extra.spawn_radius = s.radius;
        // The spawn's own AI tree (`spawn+0x109c`, 0x00535fc8) or the default list.
        cs.ai_root = cw_sim::behaviors_more::spawn_ai_behaviors(s.ai.as_ref(), &e, world.time_of_day);
        entities.insert(id, e);
        states.insert(id, cs);
        world.creature_ids.insert(id);
    }
}

/// Packet 11: the zone's modified blocks, ground items and statics, when it is loaded.
pub fn zone_discovered(world: &World, zx: i32, zy: i32, out: &mut ServerUpdate) {
    if !(0..0x10000).contains(&zx) || !(0..0x10000).contains(&zy) {
        return;
    }
    let Some(zone) = world.zone(zx, zy) else { return };
    for m in &zone.modified {
        out.block_actions.push(BlockAction { x: m.x, y: m.y, z: m.z, block: m.block, time: m.time });
    }
    out.zone_items.push(cw_net::packet::ZoneItems { zone_x: zone.x, zone_y: zone.y, items: zone.items.iter().map(ground_item_record).collect() });
    for (k, s) in zone.statics.iter().enumerate() {
        out.statics.push(static_record(zx, zy, k as i32, s));
    }
}

/// Packet 12: one mission record per cell of a loaded region, `i` outer and `j` inner.
pub fn region_discovered(world: &World, rx: i32, ry: i32, out: &mut ServerUpdate) {
    let Some(region) = world.region(rx, ry) else { return };
    for i in 0..8 {
        for j in 0..8 {
            let (cx, cy) = (rx * 8 + i, ry * 8 + j);
            if !(0..0x2000).contains(&cx) || !(0..0x2000).contains(&cy) {
                continue;
            }
            out.missions.push(to_packet(&region.cells[(i * 8 + j) as usize].mission_record(cx, cy)));
        }
    }
}
