//! The client side of the protocol (Cube.exe): `GameController::connect` 0x0046fc50, the send
//! thread 0x00469c10, the receive thread 0x0046b740 with every packet handler, and
//! `GameController::disconnect` 0x004719f0.
//!
//! Tier A for the bytes on the wire (every layout comes from `cw-net`), Tier B for timing
//! (the 20 ms send pacing, the once-a-second discovery reports).
//!
//! # Threads and locks
//!
//! The original runs two threads per connection, started by `connect` through `startThread`
//! (receive first, then send; both `SetThreadPriority(-1)`, below normal, which `std::thread`
//! cannot express). They share the `GameController` with the main thread under three critical
//! sections; this module mirrors the two it takes:
//!
//! - **A, `World::lock`** (0x00601cb0 / 0x00601e90 on the `cube::World` at
//!   `GameController+0x2e4`): the creature map (`+0x2e8`) with each creature's entity block and
//!   render fields, the incoming `UpdateLists` (`+0x800630..+0x800698`), the received chat
//!   (`+0x1000e58`), the day and time (`+0x800444`/`+0x800440`), the measured server tick
//!   (`0x0076b048`), the airship map (`+0x2f0`), the three outgoing queues
//!   (`+0x8006ec`/`+0x8006f4`/`+0x8006fc`), the chat to send (`+0x1000e60`) and the local
//!   player's Interact queue (`creature+0x130c`). Here: [`NetShared::world`] ([`NetWorld`]).
//! - **B, the critical section at `GameController+0x8005d0`**, shared with the zone and
//!   landscape threads: the discovered zone and region lists (`+0x2cc`, `+0x2d4`) that the send
//!   thread reports once a second. Here: [`NetShared::discovery`] ([`Discovery`]).
//! - C, the critical section at `+0x800600`, is the chunk mesher's; the network never takes it.
//!
//! The flags the threads poll without a lock are atomics: `connected` (`+0x800585`),
//! `connecting` (`+0x398`) and whether the socket is open (`+0x8006cc != 0`).
//!
//! # Remote entity smoothing
//!
//! The receive side decides, per update of a creature other than the local player
//! (0x0046bcc3..0x0046bdf2):
//!
//! - **snap** when the creature is new, its spawn position (`entity+0x1b0`) changed, or its
//!   HP before the update is `<= 0` (`comiss 0, hp; jae`, so a NaN HP smooths): the smoothing
//!   switch `creature+0x1398` is set to 0, the inventory (`+0x11dc`) is cleared, the render
//!   position `+0x1350` and render rotation `+0x1374` take the new entity position and
//!   rotation;
//! - **smooth** otherwise: `+0x1398 = 60`, the render position and rotation are left alone.
//!
//! Either way the previous render position, rotation and aim are copied to `+0x1338`,
//! `+0x1368` and `+0x1380` (nothing reads them) and the new entity block is assigned. The
//! shared tick (`World::tick`, the server's 0x00545940 range; `cw_sim::riding::follow_owner`)
//! then eases `+0x1350` towards the entity position with `lerpRepeat(0.015)` and turns
//! `+0x1374` towards the rotation while `+0x1398 > 0`, and copies them outright when it is 0.
//! **The 60 is never counted down**: a scan of every instruction of Cube.exe that addresses
//! `+0x1398` finds only the writes of 0 (constructor 0x0043b890, resets 0x0044646b and
//! 0x00447198, the two snaps here) and 60 (the two smooths here), and the tick's `cmp` at the
//! server's 0x545940. It is a switch, on from the first smoothed update until the next snap.
//! In this port the fields live in `cw_sim::combat::CreatureState` (`riding.render_pos`,
//! `riding.render_rot`, `riding.smoothing`, `aim`, `inventory`), so the client's tick reads
//! exactly what this module writes.

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cw_net::bytebuf::ByteBuffer;
use cw_net::compress::{compress, decompress};
use cw_net::entity::{ENTITY_SIZE, EntityData, FIELDS, read_delta, write_delta};
use cw_net::packet::{self, Hit, Interact, Passive, ServerUpdate, Shoot, id};
use cw_sim::combat::CreatureState;

/// `htons(0x3039)` in `connect`.
pub const PORT: u16 = 12345;

/// The version `connect` sends after packet 0x11 (`mov [local], 3`).
pub const CLIENT_VERSION: u32 = packet::PROTOCOL_VERSION;

/// Size of an airship record of packet 3 (`n * 0x78` bytes).
pub const AIRSHIP_SIZE: usize = 0x78;

/// The colour of the client's own chat lines ("Connected.", "Disconnected."): the four floats
/// `connect` and `disconnect` pass to the chat widget (0x0043ab30).
pub const SYSTEM_CHAT_COLOUR: [f32; 4] = [1.0, 0.5, 0.2, 1.0];

/// `timeGetTime()` (WINMM, IAT 0x006fc5c8): milliseconds, wrapping. Most uses only take
/// differences, but the constructor's first reading is the start screen's world seed
/// (0x0045a72d) and "Start Menu" reuses it (0x0047e126), so the origin must differ between
/// launches. The original counts from boot; the port starts from the wall clock's time of
/// day read once (no portable uptime in std; kept under a day, like a machine booted hours
/// ago, so the effects that feed the reading into floats, `real_time_ms`, keep their
/// precision), advanced by a monotonic `Instant` so differences never jump.
pub fn time_get_time() -> u32 {
    static START: OnceLock<(Instant, u32)> = OnceLock::new();
    let (start, origin) = *START.get_or_init(|| {
        let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        (Instant::now(), time_origin_ms(since))
    });
    origin.wrapping_add(start.elapsed().as_millis() as u32)
}

/// A wall-clock reading as milliseconds into its (UTC) day, [`time_get_time`]'s origin.
fn time_origin_ms(since_epoch: Duration) -> u32 {
    (since_epoch.as_millis() % 86_400_000) as u32
}

/// A chat line received in packet 10 (`std::list<ChatEntry>` at `GameController+0x1000e58`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatLine {
    pub sender: i64,
    pub text: Vec<u16>,
}

/// The fields the receive thread saves before it changes a creature's render state and that
/// nothing reads afterwards: `+0x1338` (render position), `+0x1368` (render rotation; packet 1
/// stores the render position scaled to blocks there instead), `+0x1380` (aim, packet 0 only).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RenderSnapshot {
    pub pos: [i64; 3],
    pub rot: [f32; 3],
    pub aim: [f32; 3],
}

/// A world reload the receive thread asks for when packet 15 names a world the client does not
/// have loaded (0x0046b907..0x0046ba60: `GameController::loadWorld` 0x0046f620 with the name
/// `online_<seed>`, then the widgets at `+0x800884` shown and `+0x800880`, `+0x800888`,
/// `+0x80088c` hidden). The original calls the loader on the receive thread; the controller
/// does it here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldRequest {
    pub seed: u32,
    pub name: String,
}

/// Everything the network threads touch under lock A (`World::lock`). The controller owns
/// the rest of the world; it keeps the creature map here so that the receive thread, the tick
/// and the send thread see one copy.
#[derive(Debug, Clone, Default)]
pub struct NetWorld {
    /// The creature map (`world+4`, `GameController+0x2e8`): id → entity block
    /// (`creature+0x10`).
    pub entities: BTreeMap<i64, EntityData>,
    /// The fields of each creature outside the entity block, as the shared tick keeps them.
    pub states: BTreeMap<i64, CreatureState>,
    /// `+0x1338`/`+0x1368`/`+0x1380`, written by the receive thread only.
    pub render_snapshots: BTreeMap<i64, RenderSnapshot>,
    /// The id of the local player's creature (`GameController+0x8006d0`, its `+8`).
    pub local_player: i64,
    /// The incoming `UpdateLists` (`GameController+0x800630`): each packet 4 is appended here
    /// (0x0046cf..: section *n* spliced onto the list of its slot) for `update` to consume.
    pub incoming: ServerUpdate,
    /// Received chat (`+0x1000e58`).
    pub chat_in: VecDeque<ChatLine>,
    /// Packet 5: `+0x800444` day and `+0x800440` time of day in ms.
    pub day: u32,
    pub time_of_day: u32,
    /// `0x0076b048`: milliseconds between the last two packet 2 (UpdateFinished).
    pub server_tick_ms: u32,
    /// Packet 3 records by id (`GameController+0x2f0`, objects of 0xa0 bytes whose first 0x78
    /// are the record; the server never sends packet 3).
    pub airships: BTreeMap<i64, Box<[u8; AIRSHIP_SIZE]>>,
    /// Outgoing queues, one packet each per element: `+0x8006ec` hits (packet 7),
    /// `+0x8006f4` passives (8), `+0x8006fc` shoots (9); see [`NetWorld::queue_tick_output`].
    pub out_hits: Vec<Hit>,
    pub out_passives: Vec<Passive>,
    pub out_shoots: Vec<Shoot>,
    /// Chat to send (`+0x1000e60`), one message per send pass.
    pub chat_out: VecDeque<Vec<u16>>,
    /// The local player's Interact queue (`creature+0x130c`), one record per send pass.
    pub interacts: VecDeque<Interact>,
    /// The loaded world's seed and name (`GameController+0x800a50`, `+0x800a54`), set by the
    /// controller; packet 15 compares against them.
    pub world_seed: u32,
    pub world_name: String,
    /// Set by packet 15, taken by the controller.
    pub world_request: Option<WorldRequest>,
    /// Set by a successful join (0x0047040e..0x0047053d): the controller clears every loaded
    /// region's 64 mission cells (`region+0x14044`, 0x68 bytes each), sets `region+8`, and marks
    /// every zone of the region table dirty (`zone+0x74`) for remeshing, then clears the flag.
    pub regions_reset: bool,
}

impl NetWorld {
    /// `update` 0x0049c02f..0x0049c254: after the tick, when the socket is open, the tick's
    /// output hits (slot 0, 0x48 bytes), passives (slot 11, 0x28) and shoots (slot 4, 0x70)
    /// whose first eight bytes are the local player's id go to the send queues
    /// `+0x8006ec` (packet 7), `+0x8006f4` (packet 8) and `+0x8006fc` (packet 9). Traced from
    /// the stack offsets of the three loops against the tick's output `UpdateLists` at
    /// `esp+0x224` (slots at `+0`, `+0x58`, `+0x20`) and the push helpers 0x00486290,
    /// 0x004460a0, 0x004861a0.
    pub fn queue_tick_output(&mut self, socket_open: bool, hits: &[Hit], passives: &[Passive], shoots: &[Shoot]) {
        if !socket_open {
            return;
        }
        let me = self.local_player;
        let from_me = |b: &[u8]| i64::from_le_bytes(b[0..8].try_into().unwrap()) == me;
        self.out_hits.extend(hits.iter().filter(|h| from_me(&h.0)).cloned());
        self.out_passives.extend(passives.iter().filter(|p| from_me(&p.0)).cloned());
        self.out_shoots.extend(shoots.iter().filter(|s| from_me(&s.0)).cloned());
    }

    /// Chat typed by the player (`onKeyDown` 0x0047e1b0 pushes onto `+0x1000e60`).
    pub fn queue_chat(&mut self, text: Vec<u16>) {
        self.chat_out.push_back(text);
    }

    /// An interaction of the local player (`creature+0x130c`).
    pub fn queue_interact(&mut self, record: Interact) {
        self.interacts.push_back(record);
    }

    /// `World::findEntity` + `new Creature(id)` (0x0043b690) + `map[id] = c`: the creature,
    /// made if missing. Returns whether it was made.
    fn find_or_create(&mut self, eid: i64) -> bool {
        if self.entities.contains_key(&eid) {
            return false;
        }
        self.entities.insert(eid, EntityData::new_creature());
        self.states.insert(eid, CreatureState::default());
        true
    }

    /// Deletes a creature and erases it from the map (virtual dtor + `map::erase` 0x0043ede0).
    fn remove_creature(&mut self, eid: i64) {
        self.entities.remove(&eid);
        self.states.remove(&eid);
        self.render_snapshots.remove(&eid);
    }
}

/// The discovered zone and region lists the send thread reports (`GameController+0x2cc`,
/// `+0x2d4`), `(x, y)` in zone and region units. Filled by the zone thread (not ported here).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Discovery {
    pub zones: Vec<(i32, i32)>,
    pub regions: Vec<(i32, i32)>,
}

/// The state the network threads share with the controller.
#[derive(Debug, Default)]
pub struct NetShared {
    /// Lock A, `World::lock`.
    pub world: Mutex<NetWorld>,
    /// Lock B, the critical section at `GameController+0x8005d0`.
    pub discovery: Mutex<Discovery>,
    /// `GameController+0x800585`: the threads run while it is set.
    pub connected: AtomicBool,
    /// `GameController+0x398`.
    pub connecting: AtomicBool,
    /// `GameController+0x8006cc != 0`.
    pub socket_open: AtomicBool,
}

impl NetShared {
    pub fn new() -> Arc<NetShared> {
        Arc::new(NetShared::default())
    }

    fn lock_world(&self) -> std::sync::MutexGuard<'_, NetWorld> {
        self.world.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn lock_discovery(&self) -> std::sync::MutexGuard<'_, Discovery> {
        self.discovery.lock().unwrap_or_else(|e| e.into_inner())
    }
}

// ---------------------------------------------------------------------------------------------
// Connecting
// ---------------------------------------------------------------------------------------------

/// The server's answer to the version handshake.
#[derive(Debug, Clone, PartialEq)]
pub enum JoinReply {
    /// `0x10`, then `u32 0`, `i64 id`, 0x1168 bytes of entity data.
    Joined { id: i64, entity: Box<EntityData> },
    /// `0x10` followed by a word other than 0: `connect` reads nothing more, starts no thread
    /// and leaves the socket open (0x00470112..).
    JoinWithoutZero(u32),
    /// `0x11` and the server's version.
    VersionMismatch(u32),
    /// `0x12`.
    ServerFull,
    /// Any other id. Server.exe answers a wrong version with the bare word 3, which lands here.
    Unexpected(u32),
}

/// Why `connect` failed, with the text it shows (`0x00636ad0` on the status label).
#[derive(Debug)]
pub enum ConnectError {
    /// Empty host: `connect` returns at once (`param_2[4] == 0`).
    EmptyHost,
    /// `connect()` failed.
    CouldNotConnect(io::Error),
    /// A `send` or `recv` failed during the handshake.
    ConnectionError(io::Error),
    VersionMismatch { server: u32 },
    ServerFull,
    Unexpected(u32),
    JoinWithoutZero(u32),
}

impl ConnectError {
    /// The message the original shows, or `None` where it shows nothing.
    pub fn message(&self) -> Option<String> {
        match self {
            ConnectError::EmptyHost | ConnectError::JoinWithoutZero(_) => None,
            ConnectError::CouldNotConnect(_) => Some("Error: could not connect to server.\n".to_string()),
            ConnectError::ConnectionError(_) => Some("Connection error.".to_string()),
            // `wostream << double` twice: the server's version * 0.01, the client's 0.03.
            ConnectError::VersionMismatch { server } => Some(format!(
                "Error: Server has different version ({}) than client ({})\n",
                fmt_g6(f64::from(*server as i32) * 0.01),
                fmt_g6(0.03)
            )),
            ConnectError::ServerFull => Some("Error: Server is full.\n".to_string()),
            // The string at the default branch has no period.
            ConnectError::Unexpected(_) => Some("Connection error".to_string()),
        }
    }
}

fn read_u32<R: Read>(r: &mut R) -> io::Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}

fn read_i64<R: Read>(r: &mut R) -> io::Result<i64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(i64::from_le_bytes(b))
}

fn read_vec<R: Read>(r: &mut R, n: usize) -> io::Result<Vec<u8>> {
    let mut v = vec![0u8; n];
    r.read_exact(&mut v)?;
    Ok(v)
}

/// The handshake part of `connect` 0x0046fc50 on an open stream: `u32 0x11` and `u32 3`
/// (two `send` calls), then the reply id and what follows it.
pub fn handshake<S: Read + Write>(s: &mut S) -> Result<JoinReply, ConnectError> {
    s.write_all(&id::CLIENT_VERSION.to_le_bytes()).map_err(ConnectError::ConnectionError)?;
    s.write_all(&CLIENT_VERSION.to_le_bytes()).map_err(ConnectError::ConnectionError)?;
    let reply = read_u32(s).map_err(ConnectError::ConnectionError)?;
    match reply {
        id::CLIENT_VERSION => {
            let v = read_u32(s).map_err(ConnectError::ConnectionError)?;
            Ok(JoinReply::VersionMismatch(v))
        }
        id::SERVER_FULL => Ok(JoinReply::ServerFull),
        id::JOIN => {
            let zero = read_u32(s).map_err(ConnectError::ConnectionError)?;
            if zero != 0 {
                return Ok(JoinReply::JoinWithoutZero(zero));
            }
            let eid = read_i64(s).map_err(ConnectError::ConnectionError)?;
            let bytes = read_vec(s, ENTITY_SIZE).map_err(ConnectError::ConnectionError)?;
            Ok(JoinReply::Joined { id: eid, entity: Box::new(EntityData::from_slice(&bytes).unwrap()) })
        }
        other => Ok(JoinReply::Unexpected(other)),
    }
}

/// `inet_addr(host)`, else the first address of `gethostbyname(host)`, on port 12345.
pub fn resolve(host: &str) -> Option<SocketAddr> {
    if let Ok(ip) = host.parse::<Ipv4Addr>() {
        return Some(SocketAddr::from((ip, PORT)));
    }
    (host, PORT).to_socket_addrs().ok()?.find(|a| a.is_ipv4())
}

/// The join of 0x00470206..0x0047053d under lock A: the local player keeps its entity block,
/// inventory and riding stamina (`+0x10`, `+0x11dc`, `+0x1198`; also the `+0x1d28` object and
/// `+0x119c`, which this port does not model), every creature is deleted, and the player is
/// made again under the id the server gave. The 0x1168 bytes the server sent are discarded.
/// Runs only for `id >= 0`.
pub fn apply_join(world: &mut NetWorld, new_id: i64) {
    let old = world.local_player;
    let entity = world.entities.get(&old).cloned().unwrap_or_else(EntityData::new_creature);
    let old_state = world.states.get(&old).cloned().unwrap_or_default();
    world.entities.clear();
    world.states.clear();
    world.render_snapshots.clear();
    let mut st = CreatureState { inventory: old_state.inventory, ..CreatureState::default() };
    st.riding.ride_stamina = old_state.riding.ride_stamina;
    world.entities.insert(new_id, entity);
    world.states.insert(new_id, st);
    world.local_player = new_id;
    world.regions_reset = true;
}

/// A live connection: the socket and the two threads.
pub struct Connection {
    stream: TcpStream,
    receive: Option<JoinHandle<()>>,
    send: Option<JoinHandle<()>>,
}

/// `GameController::connect` 0x0046fc50. On success the "Connected.\n" chat line is the
/// caller's to show (the original adds it as soon as the 0x10 arrives).
pub fn connect(host: &str, shared: &Arc<NetShared>) -> Result<Connection, ConnectError> {
    if host.is_empty() {
        return Err(ConnectError::EmptyHost);
    }
    shared.connecting.store(true, Ordering::SeqCst);
    let fail = |e: ConnectError| {
        // closesocket, `+0x8006cc = 0`, `+0x398 = 0`, the start menu shown again (+0x800894).
        shared.connecting.store(false, Ordering::SeqCst);
        shared.socket_open.store(false, Ordering::SeqCst);
        Err(e)
    };
    // `inet_addr` failing and `gethostbyname` failing leave INADDR_NONE, whose `connect` fails.
    let addr = resolve(host).unwrap_or_else(|| SocketAddr::from((Ipv4Addr::BROADCAST, PORT)));
    let mut stream = match TcpStream::connect(addr) {
        Ok(s) => s,
        Err(e) => return fail(ConnectError::CouldNotConnect(e)),
    };
    shared.socket_open.store(true, Ordering::SeqCst);
    let reply = match handshake(&mut stream) {
        Ok(r) => r,
        Err(e) => return fail(e),
    };
    let eid = match reply {
        JoinReply::Joined { id, .. } => id,
        JoinReply::VersionMismatch(v) => return fail(ConnectError::VersionMismatch { server: v }),
        JoinReply::ServerFull => return fail(ConnectError::ServerFull),
        JoinReply::Unexpected(v) => return fail(ConnectError::Unexpected(v)),
        // The original returns with the socket open and `connecting` still set; nothing reads
        // from it afterwards, so the port closes it.
        JoinReply::JoinWithoutZero(v) => return fail(ConnectError::JoinWithoutZero(v)),
    };
    if eid >= 0 {
        apply_join(&mut shared.lock_world(), eid);
    }
    shared.connected.store(true, Ordering::SeqCst);
    let (Ok(rs), Ok(ss)) = (stream.try_clone(), stream.try_clone()) else {
        shared.connected.store(false, Ordering::SeqCst);
        return fail(ConnectError::ConnectionError(io::Error::other("socket clone failed")));
    };
    let sh = Arc::clone(shared);
    let receive = std::thread::Builder::new()
        .name("receiveLoop".into())
        .spawn(move || receive_loop(&mut io::BufReader::new(rs), &sh))
        .map_err(ConnectError::ConnectionError)?;
    let sh = Arc::clone(shared);
    let send = std::thread::Builder::new()
        .name("sendLoop".into())
        .spawn(move || send_loop(ss, &sh))
        .map_err(ConnectError::ConnectionError)?;
    Ok(Connection { stream, receive: Some(receive), send: Some(send) })
}

impl Connection {
    /// `GameController::disconnect` 0x004719f0, the network part: `connected = 0`, the socket
    /// closed (which ends the blocked `recv`), `connecting = 0`, then both threads joined. The
    /// caller then saves the character (0x00487520), shows "Disconnected.\n" in
    /// [`SYSTEM_CHAT_COLOUR`] and reloads the offline world (`loadWorld` 0x0046f620 with
    /// `+0x800448`, `+0x378`).
    pub fn disconnect(mut self, shared: &NetShared) {
        if !shared.connected.load(Ordering::SeqCst) {
            // The original does nothing when not connected; the threads have ended already.
            self.join();
            return;
        }
        shared.connected.store(false, Ordering::SeqCst);
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
        shared.socket_open.store(false, Ordering::SeqCst);
        shared.connecting.store(false, Ordering::SeqCst);
        self.join();
    }

    fn join(&mut self) {
        if let Some(h) = self.receive.take() {
            let _ = h.join();
        }
        if let Some(h) = self.send.take() {
            let _ = h.join();
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Receiving
// ---------------------------------------------------------------------------------------------

/// Thread-local state of `receiveLoop`.
#[derive(Debug, Default)]
pub struct ReceiveState {
    /// Ids updated by packet 0 since the last packet 2 (the `std::set` at `ebp-0x11f8`).
    pub received: BTreeSet<i64>,
    /// `timeGetTime()` at the last packet 2, 0 before the first.
    pub last_finished: u32,
}

/// Why the receive loop stopped.
#[derive(Debug)]
pub enum ReceiveEnd {
    /// A `recv` failed. The original only stops on -1; a `recv` of 0 (orderly close) is
    /// treated the same here.
    Io(io::Error),
    /// Packet 0 with length 0 (0x0046bb2c).
    ZeroLengthUpdate,
}

impl From<io::Error> for ReceiveEnd {
    fn from(e: io::Error) -> Self {
        ReceiveEnd::Io(e)
    }
}

/// `receiveLoop` 0x0046b740: packets until a `recv` fails, a zero-length entity update, or
/// `connected` is cleared (checked after each packet).
pub fn receive_loop<R: Read>(r: &mut R, shared: &NetShared) {
    let mut st = ReceiveState::default();
    if !shared.connected.load(Ordering::SeqCst) {
        return;
    }
    loop {
        if receive_packet(r, shared, &mut st).is_err() {
            break;
        }
        if !shared.connected.load(Ordering::SeqCst) {
            break;
        }
    }
}

/// One packet of `receiveLoop` (jump tables 0x0046d180 / 0x0046d1a4): ids 0..15 as listed in
/// the arms; 6..9, 11..14 and everything above 15 carry nothing and are skipped.
pub fn receive_packet<R: Read>(r: &mut R, shared: &NetShared, st: &mut ReceiveState) -> Result<(), ReceiveEnd> {
    let pid = read_u32(r)?;
    if pid == id::UPDATE_FINISHED {
        // 0x0046b85a: the interval between UpdateFinished packets, under lock A.
        let now = time_get_time();
        let mut w = shared.lock_world();
        if st.last_finished != 0 {
            w.server_tick_ms = now.wrapping_sub(st.last_finished);
        }
        drop(w);
        st.last_finished = now;
    }
    match pid {
        id::ENTITY_UPDATE => receive_entity_update(r, shared, st),
        1 => receive_multiple_entity_update(r, shared),
        id::UPDATE_FINISHED => {
            update_finished(shared, st);
            Ok(())
        }
        3 => receive_airships(r, shared),
        id::SERVER_UPDATE => receive_server_update(r, shared),
        id::CURRENT_TIME => {
            // 0x0046cb42: day, then time; stored under lock A.
            let day = read_u32(r)?;
            let time = read_u32(r)?;
            let mut w = shared.lock_world();
            w.time_of_day = time;
            w.day = day;
            Ok(())
        }
        id::CHAT => receive_chat(r, shared),
        id::SEED => receive_seed(r, shared),
        _ => Ok(()),
    }
}

/// Packet 0 (0x0046baad): `u32 len`, `len` bytes of zlib holding `i64 id` and a masked delta.
fn receive_entity_update<R: Read>(r: &mut R, shared: &NetShared, st: &mut ReceiveState) -> Result<(), ReceiveEnd> {
    let len = read_u32(r)? as usize;
    if len == 0 {
        return Err(ReceiveEnd::ZeroLengthUpdate);
    }
    let z = read_vec(r, len)?;
    // `ByteBuffer::decompress`: the partial output is kept on an inflate error.
    let body = decompress(&z).unwrap_or_else(|(partial, _)| partial);
    let mut buf = ByteBuffer::from_vec(body);
    let eid = buf.read_i64().unwrap_or(0);
    apply_entity_update(shared, eid, &mut buf);
    st.received.insert(eid);
    Ok(())
}

/// The apply half of packet 0: copy under lock A, `readEntityDelta` 0x004ccfa0 without the
/// lock, then the smoothing decision and the assignment under lock A (0x0046bb7b..0x0046bdf2).
pub fn apply_entity_update(shared: &NetShared, eid: i64, buf: &mut ByteBuffer) {
    let (created, mut temp) = {
        let mut w = shared.lock_world();
        let created = w.find_or_create(eid);
        (created, w.entities[&eid].clone())
    };
    read_delta(buf, &mut temp);
    let mut w = shared.lock_world();
    if eid == w.local_player {
        return;
    }
    if !w.entities.contains_key(&eid) {
        return;
    }
    let old = &w.entities[&eid];
    // `Vec3i64::operator!=` 0x0042c680 on the spawn position (`creature+0x1c0`).
    let respawned = old.0[0x1b0..0x1c8] != temp.0[0x1b0..0x1c8];
    let hp = f32_at(old, 0x15c);
    let snap = created || respawned || hp <= 0.0;
    let new_pos = pos_at(&temp, 0);
    let new_rot = vec3f_at(&temp, 0x18);
    let st = w.states.entry(eid).or_default();
    let snapshot = if snap {
        st.riding.smoothing = 0;
        // 0x0043e630 on `+0x11dc`.
        st.inventory.pages.clear();
        st.riding.render_pos = new_pos;
        st.riding.render_rot = new_rot;
        RenderSnapshot { pos: st.riding.render_pos, rot: st.riding.render_rot, aim: [0.0; 3] }
    } else {
        st.riding.smoothing = 0x3c;
        RenderSnapshot { pos: st.riding.render_pos, rot: st.riding.render_rot, aim: st.aim }
    };
    let entry = w.render_snapshots.entry(eid).or_default();
    entry.pos = snapshot.pos;
    entry.rot = snapshot.rot;
    if !snap {
        entry.aim = snapshot.aim;
    }
    // `EntityData::operator=` 0x0044b040.
    w.entities.insert(eid, temp);
}

/// Packet 2 (0x0046be25): every creature other than the local player that got no packet 0
/// since the last packet 2 is deleted, under lock A; then the received set is cleared.
fn update_finished(shared: &NetShared, st: &mut ReceiveState) {
    let mut w = shared.lock_world();
    let me = w.local_player;
    let gone: Vec<i64> = w.entities.keys().copied().filter(|k| *k != me && !st.received.contains(k)).collect();
    for k in gone {
        w.remove_creature(k);
    }
    drop(w);
    st.received.clear();
}

/// Reads one masked entity delta straight from the stream, as `0x004cd3e0` does field by
/// field (`u64 mask`, then each set field of the 48 in bit order), into `target`.
pub fn read_delta_from_stream<R: Read>(r: &mut R, target: &mut EntityData) -> io::Result<()> {
    let mut m = [0u8; 8];
    r.read_exact(&mut m)?;
    let mask = u64::from_le_bytes(m);
    for (bit, f) in FIELDS.iter().enumerate() {
        if mask & (1u64 << bit) != 0 {
            r.read_exact(&mut target.0[f.offset..f.offset + f.size])?;
        }
    }
    Ok(())
}

/// Packet 1 (0x0046c296), never sent by Server.exe: `u32 n`, then `n` times `i64 id` and an
/// uncompressed masked delta. Applied against a snapshot of every creature's entity block;
/// afterwards every creature not named (the local player excepted) is deleted, and the named
/// ones are made, snapped or smoothed as in packet 0.
fn receive_multiple_entity_update<R: Read>(r: &mut R, shared: &NetShared) -> Result<(), ReceiveEnd> {
    let n = read_u32(r)? as i32;
    // The map at `ebp-0x1208`: cleared, then filled with every creature's block under lock A.
    let mut cache: BTreeMap<i64, EntityData> = shared.lock_world().entities.clone();
    let mut named = BTreeSet::new();
    for _ in 0..n.max(0) {
        let eid = read_i64(r)?;
        // `map::operator[]` default-constructs (`EntityData::EntityData` 0x00407020).
        let e = cache.entry(eid).or_insert_with(EntityData::constructed);
        read_delta_from_stream(r, e)?;
        named.insert(eid);
    }
    let mut w = shared.lock_world();
    let me = w.local_player;
    named.insert(me);
    let gone: Vec<i64> = w.entities.keys().copied().filter(|k| !named.contains(k)).collect();
    for k in gone {
        w.remove_creature(k);
    }
    const INV: f32 = 1.525_878_9e-5;
    for &eid in &named {
        let created = w.find_or_create(eid);
        if eid == me {
            continue;
        }
        let data = cache.entry(eid).or_insert_with(EntityData::constructed).clone();
        let old = &w.entities[&eid];
        let same_spawn = old.0[0x1b0..0x1c8] == data.0[0x1b0..0x1c8];
        let hp = f32_at(old, 0x15c);
        let st = w.states.entry(eid).or_default();
        let snap;
        if created || !same_spawn || hp <= 0.0 {
            // 0x0046c7b1
            st.riding.smoothing = 0;
            st.inventory.pages.clear();
            st.riding.render_pos = pos_at(&data, 0);
            st.riding.render_rot = vec3f_at(&data, 0x18);
            // `+0x1368` gets the render position in blocks here, not the rotation.
            let p = st.riding.render_pos;
            snap = RenderSnapshot { pos: p, rot: [p[0] as f32 * INV, p[1] as f32 * INV, p[2] as f32 * INV], aim: [0.0; 3] };
            let entry = w.render_snapshots.entry(eid).or_default();
            entry.pos = snap.pos;
            entry.rot = snap.rot;
        } else {
            st.riding.smoothing = 0x3c;
            snap = RenderSnapshot { pos: st.riding.render_pos, rot: st.riding.render_rot, aim: st.aim };
            w.render_snapshots.insert(eid, snap);
        }
        w.entities.insert(eid, data);
    }
    Ok(())
}

/// Packet 3 (0x0046bfb0), never sent by Server.exe: `u32 n`, `n` records of 0x78 bytes keyed
/// by their first `i64`. Under lock A the map's objects are zeroed at their first eight
/// bytes, each record is copied into the object of its id (made if missing, 0x00466510 /
/// 0x00468790), and objects still zero there are deleted.
fn receive_airships<R: Read>(r: &mut R, shared: &NetShared) -> Result<(), ReceiveEnd> {
    let n = read_u32(r)? as i32;
    if n <= 0 {
        return Ok(());
    }
    let bytes = read_vec(r, n as usize * AIRSHIP_SIZE)?;
    let mut w = shared.lock_world();
    for v in w.airships.values_mut() {
        v[0..8].fill(0);
    }
    for rec in bytes.as_chunks::<AIRSHIP_SIZE>().0 {
        let key = i64::from_le_bytes(rec[0..8].try_into().unwrap());
        let obj = w.airships.entry(key).or_insert_with(|| Box::new([0; AIRSHIP_SIZE]));
        obj.copy_from_slice(rec);
    }
    w.airships.retain(|_, v| v[0..8] != [0; 8]);
    Ok(())
}

/// Appends a ServerUpdate to the incoming lists, section by section (`splice` onto
/// `GameController+0x800630 + 8 * slot`, 0x0046cfb0..).
pub fn append_update(acc: &mut ServerUpdate, u: ServerUpdate) {
    acc.block_actions.extend(u.block_actions);
    acc.hits.extend(u.hits);
    acc.particles.extend(u.particles);
    acc.sounds.extend(u.sounds);
    acc.shoots.extend(u.shoots);
    acc.statics.extend(u.statics);
    acc.zone_items.extend(u.zone_items);
    acc.items_8.extend(u.items_8);
    acc.pickups.extend(u.pickups);
    acc.kills.extend(u.kills);
    acc.damage.extend(u.damage);
    acc.passives.extend(u.passives);
    acc.missions.extend(u.missions);
}

/// Packet 4 (0x0046cc0e): `u32 len` and a zlib body of thirteen sections, parsed without a
/// lock and appended to the incoming lists under lock A.
fn receive_server_update<R: Read>(r: &mut R, shared: &NetShared) -> Result<(), ReceiveEnd> {
    let len = read_u32(r)? as usize;
    let z = read_vec(r, len)?;
    let body = decompress(&z).unwrap_or_else(|(partial, _)| partial);
    // The original's section readers clamp at the end of a short body and keep going; a
    // truncated body is dropped whole here.
    if let Some(u) = ServerUpdate::read_body(&body) {
        append_update(&mut shared.lock_world().incoming, u);
    }
    Ok(())
}

/// Packet 10 (0x0046ca1e): `i64 sender`, `i32 n`, and when `n > 0` the `n` UTF-16 units,
/// pushed onto the received chat under lock A.
fn receive_chat<R: Read>(r: &mut R, shared: &NetShared) -> Result<(), ReceiveEnd> {
    let sender = read_i64(r)?;
    let n = read_u32(r)? as i32;
    if n <= 0 {
        return Ok(());
    }
    let bytes = read_vec(r, n as usize * 2)?;
    let text = bytes.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
    shared.lock_world().chat_in.push_back(ChatLine { sender, text });
    Ok(())
}

/// Packet 15 (0x0046b8be): `u32 seed`. The world is called `online_<seed>` (the seed printed
/// as a signed int); when the seed or the name differs from the loaded world, a reload is
/// requested.
fn receive_seed<R: Read>(r: &mut R, shared: &NetShared) -> Result<(), ReceiveEnd> {
    let seed = read_u32(r)?;
    let name = format!("online_{}", seed as i32);
    let mut w = shared.lock_world();
    if seed != w.world_seed || w.world_name != name {
        w.world_request = Some(WorldRequest { seed, name });
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Sending
// ---------------------------------------------------------------------------------------------

/// Thread-local state of `sendLoop`.
#[derive(Debug, Clone)]
pub struct SendState {
    /// The entity block last sent (`ebp-0x12c0`, default-constructed at thread start).
    pub prev: EntityData,
    /// Send every field (`ebp-0x247c`), set until the first successful packet 0.
    pub full: bool,
    /// `timeGetTime()` of the last discovery report (thread start at first).
    pub last_discovery: u32,
    /// The pacing timestamp (a global at 0x0076b07c, 0 at start).
    pub last_pass: u32,
}

impl SendState {
    pub fn new(now: u32) -> SendState {
        SendState { prev: EntityData::constructed(), full: true, last_discovery: now, last_pass: 0 }
    }
}

/// One pass of `sendLoop` 0x00469c10; returns true when a `send` failed. In order:
///
/// 1. `u32 0`, the packet 0 id, before anything is gathered;
/// 2. under lock A: the three queues copied and cleared, one chat line and one Interact
///    popped, the local player's id and entity block copied;
/// 3. packet 0's `u32 len` and the zlib of `i64 id` + `writeEntityDelta(prev, cur, full)`;
///    after it went out, `full` is cleared and `prev = cur`;
/// 4. packet 10 when the chat line is not empty, packet 6 when there was an Interact;
/// 5. packets 7, 8, 9 for each queued hit, passive and shoot, in that order;
/// 6. under lock B the discovered lists copied; when more than 1000 ms passed since the last
///    report, packet 11 for every zone and 12 for every region (a failure marks the pass
///    failed but the regions are still tried);
/// 7. the 20 ms pacing: `if (now - last < 20) Sleep(last - now + 20); last = now`, with `now`
///    read before the reports.
pub fn send_pass<W: Write>(w: &mut W, shared: &NetShared, st: &mut SendState) -> bool {
    if w.write_all(&id::ENTITY_UPDATE.to_le_bytes()).is_err() {
        return true;
    }
    let (hits, passives, shoots, chat, interact, me, cur) = {
        let mut g = shared.lock_world();
        let hits = std::mem::take(&mut g.out_hits);
        let passives = std::mem::take(&mut g.out_passives);
        let shoots = std::mem::take(&mut g.out_shoots);
        let chat = g.chat_out.pop_front().unwrap_or_default();
        let interact = g.interacts.pop_front();
        let me = g.local_player;
        let cur = g.entities.get(&me).cloned().unwrap_or_else(EntityData::new_creature);
        (hits, passives, shoots, chat, interact, me, cur)
    };
    let mut buf = ByteBuffer::new();
    buf.write_i64(me);
    write_delta(&mut buf, &st.prev, &cur, st.full);
    let z = compress(&buf.data);
    if w.write_all(&(z.len() as u32).to_le_bytes()).is_err() || w.write_all(&z).is_err() {
        return true;
    }
    st.full = false;
    st.prev = cur;
    let mut out = Vec::new();
    if !chat.is_empty() {
        packet::encode_client_packet(&packet::ClientPacket::Chat(chat), &mut out);
    }
    if let Some(i) = interact {
        packet::encode_client_packet(&packet::ClientPacket::Interact(Box::new(i)), &mut out);
    }
    if w.write_all(&out).is_err() {
        return true;
    }
    for h in hits {
        out.clear();
        packet::encode_client_packet(&packet::ClientPacket::Hit(Box::new(h)), &mut out);
        if w.write_all(&out).is_err() {
            return true;
        }
    }
    for p in passives {
        out.clear();
        packet::encode_client_packet(&packet::ClientPacket::Passive(Box::new(p)), &mut out);
        if w.write_all(&out).is_err() {
            return true;
        }
    }
    for s in shoots {
        out.clear();
        packet::encode_client_packet(&packet::ClientPacket::Shoot(Box::new(s)), &mut out);
        if w.write_all(&out).is_err() {
            return true;
        }
    }
    let (zones, regions) = {
        let d = shared.lock_discovery();
        (d.zones.clone(), d.regions.clone())
    };
    let now = time_get_time();
    let mut failed = false;
    if now.wrapping_sub(st.last_discovery) > 1000 {
        st.last_discovery = now;
        for &(x, y) in &zones {
            out.clear();
            packet::encode_client_packet(&packet::ClientPacket::ZoneDiscovered { x, y }, &mut out);
            if w.write_all(&out).is_err() {
                failed = true;
                break;
            }
        }
        for &(x, y) in &regions {
            out.clear();
            packet::encode_client_packet(&packet::ClientPacket::RegionDiscovered { x, y }, &mut out);
            if w.write_all(&out).is_err() {
                failed = true;
                break;
            }
        }
    }
    if now.wrapping_sub(st.last_pass) < 20 {
        std::thread::sleep(Duration::from_millis(u64::from(st.last_pass.wrapping_sub(now).wrapping_add(20))));
    }
    st.last_pass = now;
    failed
}

/// `sendLoop` 0x00469c10: passes while `connected`; after a failed `send`, if still
/// connected, the socket is closed (`+0x8006cc = 0`, `+0x398 = 0`) and the thread ends.
pub fn send_loop(mut stream: TcpStream, shared: &NetShared) {
    if !shared.connected.load(Ordering::SeqCst) {
        return;
    }
    let mut st = SendState::new(time_get_time());
    loop {
        if !send_pass(&mut stream, shared, &mut st) {
            if shared.connected.load(Ordering::SeqCst) {
                continue;
            }
            break;
        }
        if shared.connected.load(Ordering::SeqCst) {
            let _ = stream.shutdown(std::net::Shutdown::Both);
            shared.socket_open.store(false, Ordering::SeqCst);
            shared.connecting.store(false, Ordering::SeqCst);
        }
        break;
    }
}

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

fn f32_at(e: &EntityData, o: usize) -> f32 {
    f32::from_le_bytes(e.0[o..o + 4].try_into().unwrap())
}

fn pos_at(e: &EntityData, o: usize) -> [i64; 3] {
    let i = |k: usize| i64::from_le_bytes(e.0[o + 8 * k..o + 8 * k + 8].try_into().unwrap());
    [i(0), i(1), i(2)]
}

fn vec3f_at(e: &EntityData, o: usize) -> [f32; 3] {
    [f32_at(e, o), f32_at(e, o + 4), f32_at(e, o + 8)]
}

/// `wostream << double` with the default flags: `%g` with six significant digits, and the
/// three-digit exponent of the VS2012 CRT.
pub fn fmt_g6(x: f64) -> String {
    if x == 0.0 {
        return "0".to_string();
    }
    if !x.is_finite() {
        return format!("{x}");
    }
    let sci = format!("{:.5e}", x);
    let (mant, exp) = sci.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    let trim = |s: &str| -> String {
        if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s.to_string() }
    };
    if !(-4..6).contains(&exp) {
        let sign = if exp < 0 { '-' } else { '+' };
        format!("{}e{}{:03}", trim(mant), sign, exp.abs())
    } else {
        let decimals = (5 - exp).max(0) as usize;
        trim(&format!("{:.*}", decimals, x))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cw_net::packet::{BlockAction, ClientPacket, Mission, Parse, ServerPacket, parse_client_packet};

    /// `timeGetTime` counts from boot, not from the process start: its first reading (the
    /// start screen's world seed, 0x0045a72d) differs between launches.
    #[test]
    fn time_origin_is_a_clock_not_the_process_start() {
        assert_eq!(time_origin_ms(Duration::from_millis(1_234_567)), 1_234_567);
        // Milliseconds into the day.
        assert_eq!(time_origin_ms(Duration::from_millis(3 * 86_400_000 + 5)), 5);
        let a = time_origin_ms(Duration::from_secs(1_790_000_000));
        let b = time_origin_ms(Duration::from_secs(1_790_000_060));
        assert_eq!(b.wrapping_sub(a), 60_000);
    }

    /// A socket double: reads come from `input`, writes go to `output`.
    struct FakeSocket {
        input: io::Cursor<Vec<u8>>,
        output: Vec<u8>,
    }

    impl Read for FakeSocket {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.input.read(buf)
        }
    }

    impl Write for FakeSocket {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.output.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn connected_shared() -> Arc<NetShared> {
        let s = NetShared::new();
        s.connected.store(true, Ordering::SeqCst);
        s
    }

    fn parse_all(mut bytes: &[u8]) -> Vec<ClientPacket> {
        let mut out = Vec::new();
        while !bytes.is_empty() {
            match parse_client_packet(bytes) {
                Parse::Packet(p, n) => {
                    out.push(p);
                    bytes = &bytes[n..];
                }
                other => panic!("{other:?} at {} bytes left", bytes.len()),
            }
        }
        out
    }

    #[test]
    fn handshake_joins() {
        let mut entity = EntityData::new_creature();
        entity.0[0x180] = 7;
        let mut input = ServerPacket::Join { id: 3, entity: Box::new(entity.clone()) }.to_bytes();
        input.extend(ServerPacket::Seed(26879).to_bytes());
        let mut s = FakeSocket { input: io::Cursor::new(input), output: Vec::new() };
        let reply = handshake(&mut s).unwrap();
        assert_eq!(s.output, [0x11, 0, 0, 0, 3, 0, 0, 0]);
        assert_eq!(reply, JoinReply::Joined { id: 3, entity: Box::new(entity) });
        // The seed packet is left for the receive loop.
        assert_eq!(s.input.position() as usize, packet::JOIN_SIZE);
    }

    #[test]
    fn handshake_refusals() {
        let run = |input: Vec<u8>| handshake(&mut FakeSocket { input: io::Cursor::new(input), output: Vec::new() });
        // Server.exe's answer to a wrong version: the bare word 3.
        assert_eq!(run(ServerPacket::VersionMismatch.to_bytes()).unwrap(), JoinReply::Unexpected(3));
        assert_eq!(run(ServerPacket::ServerFull.to_bytes()).unwrap(), JoinReply::ServerFull);
        assert_eq!(run([0x11, 0, 0, 0, 2, 0, 0, 0].to_vec()).unwrap(), JoinReply::VersionMismatch(2));
        assert_eq!(run([0x10, 0, 0, 0, 1, 0, 0, 0].to_vec()).unwrap(), JoinReply::JoinWithoutZero(1));
        assert!(matches!(run(vec![0x10, 0]), Err(ConnectError::ConnectionError(_))));
        assert_eq!(
            ConnectError::VersionMismatch { server: 2 }.message().unwrap(),
            "Error: Server has different version (0.02) than client (0.03)\n"
        );
        assert_eq!(ConnectError::Unexpected(3).message().unwrap(), "Connection error");
    }

    #[test]
    fn join_keeps_the_player_and_drops_the_rest() {
        let mut w = NetWorld { local_player: 0, ..NetWorld::default() };
        let mut me = EntityData::new_creature();
        me.0[0x1158..0x115c].copy_from_slice(b"Zed\0");
        w.entities.insert(0, me.clone());
        w.states.insert(0, CreatureState::default());
        w.entities.insert(99, EntityData::new_creature());
        apply_join(&mut w, 4);
        assert_eq!(w.local_player, 4);
        assert_eq!(w.entities.len(), 1);
        assert_eq!(w.entities[&4], me);
        assert!(w.regions_reset);
    }

    fn entity_update(eid: i64, prev: &EntityData, cur: &EntityData, full: bool) -> Vec<u8> {
        let mut b = ByteBuffer::new();
        write_delta(&mut b, prev, cur, full);
        ServerPacket::EntityUpdate { id: eid, delta: b.data }.to_bytes()
    }

    #[test]
    fn receive_loop_applies_every_server_packet() {
        let shared = connected_shared();
        shared.lock_world().local_player = 1;
        shared.lock_world().entities.insert(1, EntityData::new_creature());
        let base = EntityData::new_creature();
        let mut a = base.clone();
        a.0[0..8].copy_from_slice(&(100i64 << 16).to_le_bytes());
        a.0[0x15c..0x160].copy_from_slice(&50.0f32.to_le_bytes());
        let mut a2 = a.clone();
        a2.0[0..8].copy_from_slice(&(101i64 << 16).to_le_bytes());
        let mut input = Vec::new();
        input.extend(ServerPacket::Seed(26879).to_bytes());
        input.extend(entity_update(7, &base, &a, true));
        input.extend(entity_update(8, &base, &a, true));
        input.extend(entity_update(1, &base, &a, true)); // the local player: ignored
        input.extend(ServerPacket::UpdateFinished.to_bytes());
        let mut u = ServerUpdate::default();
        u.block_actions.push(BlockAction { x: 1, y: 2, z: 3, block: [4, 5, 6, 7], time: -1 });
        u.sounds.push(cw_net::packet::Sound([3; 0x18]));
        u.missions.push(Mission { cell_x: 4096, cell_y: 4097, words: [1, 2, 3, 4, 5], b40: 6, b41: 7, tail: [8, 9, 10, 11] });
        input.extend(ServerPacket::CurrentTime { day: 2, time: 43_200_000 }.to_bytes());
        input.extend(ServerPacket::ServerUpdate(Box::new(u.clone())).to_bytes());
        input.extend(ServerPacket::Chat { sender: 7, text: vec![0x48, 0x69] }.to_bytes());
        input.extend(ServerPacket::Chat { sender: 7, text: vec![] }.to_bytes());
        input.extend([9, 0, 0, 0, 13, 0, 0, 0, 99, 0, 0, 0]); // ids with no payload are skipped
        // Second frame: only 7 moves; 8 is not updated and goes at UpdateFinished.
        input.extend(entity_update(7, &a, &a2, false));
        input.extend(ServerPacket::UpdateFinished.to_bytes());
        let mut r = io::Cursor::new(input);
        receive_loop(&mut r, &shared);
        let w = shared.lock_world();
        assert_eq!(w.world_request, Some(WorldRequest { seed: 26879, name: "online_26879".into() }));
        assert_eq!(w.entities.keys().copied().collect::<Vec<_>>(), vec![1, 7]);
        assert_eq!(w.entities[&7], a2);
        assert_eq!(w.entities[&1], EntityData::new_creature());
        // First update snapped (new creature), the second smoothed.
        let st = &w.states[&7];
        assert_eq!(st.riding.smoothing, 60);
        assert_eq!(st.riding.render_pos, [100 << 16, 0, 0]);
        assert_eq!((w.day, w.time_of_day), (2, 43_200_000));
        assert_eq!(w.incoming, u);
        assert_eq!(w.chat_in, VecDeque::from([ChatLine { sender: 7, text: vec![0x48, 0x69] }]));
    }

    #[test]
    fn snap_rules() {
        let shared = connected_shared();
        let base = EntityData::new_creature();
        let mut dead = base.clone();
        dead.0[0x15c..0x160].copy_from_slice(&0.0f32.to_le_bytes());
        let mut moved = dead.clone();
        moved.0[0..8].copy_from_slice(&(5i64 << 16).to_le_bytes());
        let apply = |prev: &EntityData, cur: &EntityData| {
            let mut b = ByteBuffer::new();
            write_delta(&mut b, prev, cur, false);
            b.pos = 0;
            apply_entity_update(&shared, 9, &mut b);
        };
        apply(&base, &dead);
        // Dead before the update: snap.
        apply(&dead, &moved);
        assert_eq!(shared.lock_world().states[&9].riding.smoothing, 0);
        assert_eq!(shared.lock_world().states[&9].riding.render_pos, [5 << 16, 0, 0]);
        // Alive: smooth, render position kept.
        let mut alive = moved.clone();
        alive.0[0x15c..0x160].copy_from_slice(&10.0f32.to_le_bytes());
        apply(&moved, &alive);
        let mut further = alive.clone();
        further.0[0..8].copy_from_slice(&(6i64 << 16).to_le_bytes());
        apply(&alive, &further);
        assert_eq!(shared.lock_world().states[&9].riding.smoothing, 60);
        assert_eq!(shared.lock_world().states[&9].riding.render_pos, [5 << 16, 0, 0]);
        // A new spawn position: snap.
        let mut respawn = further.clone();
        respawn.0[0x1b0] = 1;
        apply(&further, &respawn);
        assert_eq!(shared.lock_world().states[&9].riding.smoothing, 0);
        assert_eq!(shared.lock_world().states[&9].riding.render_pos, [6 << 16, 0, 0]);
    }

    #[test]
    fn zero_length_update_ends_the_loop() {
        let shared = connected_shared();
        let mut input = vec![0, 0, 0, 0, 0, 0, 0, 0];
        input.extend(ServerPacket::CurrentTime { day: 5, time: 1 }.to_bytes());
        let mut r = io::Cursor::new(input);
        receive_loop(&mut r, &shared);
        assert_eq!(shared.lock_world().day, 0);
    }

    #[test]
    fn multiple_entity_update_and_airships() {
        let shared = connected_shared();
        shared.lock_world().local_player = 1;
        shared.lock_world().entities.insert(1, EntityData::new_creature());
        shared.lock_world().entities.insert(5, EntityData::new_creature());
        let mut e = EntityData::constructed();
        e.0[0x180..0x184].copy_from_slice(&3i32.to_le_bytes());
        let mut body = Vec::new();
        body.extend(1u32.to_le_bytes());
        body.extend(6i64.to_le_bytes());
        let mut d = ByteBuffer::new();
        write_delta(&mut d, &EntityData::constructed(), &e, false);
        body.extend(&d.data);
        let mut input = 1u32.to_le_bytes().to_vec();
        input.extend(body);
        input.extend(3u32.to_le_bytes());
        input.extend(2u32.to_le_bytes());
        let mut ship = [0u8; AIRSHIP_SIZE];
        ship[0] = 42;
        input.extend(ship);
        input.extend([0u8; AIRSHIP_SIZE]); // id 0: removed again
        let mut r = io::Cursor::new(input);
        let mut st = ReceiveState::default();
        receive_packet(&mut r, &shared, &mut st).unwrap();
        receive_packet(&mut r, &shared, &mut st).unwrap();
        let w = shared.lock_world();
        assert_eq!(w.entities.keys().copied().collect::<Vec<_>>(), vec![1, 6]);
        assert_eq!(w.entities[&6], e);
        assert_eq!(w.airships.keys().copied().collect::<Vec<_>>(), vec![42]);
    }

    #[test]
    fn send_pass_order_and_bytes() {
        let shared = connected_shared();
        {
            let mut w = shared.lock_world();
            w.local_player = 3;
            let mut me = EntityData::new_creature();
            me.0[0x180] = 9;
            w.entities.insert(3, me);
            let mut hit = Hit::default();
            hit.0[0..8].copy_from_slice(&3i64.to_le_bytes());
            let mut other = Hit::default();
            other.0[0..8].copy_from_slice(&4i64.to_le_bytes());
            let mut pas = Passive::default();
            pas.0[0..8].copy_from_slice(&3i64.to_le_bytes());
            let mut sh = Shoot::default();
            sh.0[0..8].copy_from_slice(&3i64.to_le_bytes());
            w.queue_tick_output(true, &[hit.clone(), other], &[pas], &[sh]);
            w.queue_tick_output(false, &[hit], &[], &[]);
            w.queue_chat(vec![0x61, 0x62]);
            w.queue_chat(vec![0x63]);
            w.queue_interact(Interact([1; 0x12c]));
        }
        shared.lock_discovery().zones.push((32768, 32769));
        shared.lock_discovery().regions.push((512, 512));
        let mut st = SendState::new(time_get_time().wrapping_sub(2000));
        let mut out = Vec::new();
        assert!(!send_pass(&mut out, &shared, &mut st));
        let packets = parse_all(&out);
        let kinds: Vec<&'static str> = packets
            .iter()
            .map(|p| match p {
                ClientPacket::EntityUpdate { .. } => "entity",
                ClientPacket::Chat(_) => "chat",
                ClientPacket::Interact(_) => "interact",
                ClientPacket::Hit(_) => "hit",
                ClientPacket::Passive(_) => "passive",
                ClientPacket::Shoot(_) => "shoot",
                ClientPacket::ZoneDiscovered { .. } => "zone",
                ClientPacket::RegionDiscovered { .. } => "region",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, ["entity", "chat", "interact", "hit", "passive", "shoot", "zone", "region"]);
        let ClientPacket::EntityUpdate { compressed } = &packets[0] else { unreachable!() };
        let (eid, delta) = ClientPacket::decode_entity_update(compressed).unwrap();
        assert_eq!(eid, 3);
        let mut back = EntityData::constructed();
        read_delta(&mut ByteBuffer::from_vec(delta), &mut back);
        assert_eq!(back.0[0x180], 9);
        assert_eq!(packets[1], ClientPacket::Chat(vec![0x61, 0x62]));
        assert!(!st.full);
        // Second pass: nothing changed, an empty mask; the second chat line; no reports yet.
        let mut out2 = Vec::new();
        assert!(!send_pass(&mut out2, &shared, &mut st));
        let p2 = parse_all(&out2);
        assert_eq!(p2.len(), 2);
        let ClientPacket::EntityUpdate { compressed } = &p2[0] else { unreachable!() };
        assert_eq!(ClientPacket::decode_entity_update(compressed).unwrap().1, vec![0; 8]);
        assert_eq!(p2[1], ClientPacket::Chat(vec![0x63]));
    }

    #[test]
    fn send_loop_against_a_local_server() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut head = [0u8; 8];
            s.read_exact(&mut head).unwrap();
            assert_eq!(head, [0x11, 0, 0, 0, 3, 0, 0, 0]);
            s.write_all(&ServerPacket::Join { id: 2, entity: Box::new(EntityData::new_creature()) }.to_bytes()).unwrap();
            // Read the first entity update, then close.
            let mut id_len = [0u8; 8];
            s.read_exact(&mut id_len).unwrap();
            assert_eq!(&id_len[..4], &[0, 0, 0, 0]);
        });
        let shared = NetShared::new();
        shared.lock_world().entities.insert(0, EntityData::new_creature());
        let stream = TcpStream::connect(addr).unwrap();
        let mut s2 = stream.try_clone().unwrap();
        let reply = handshake(&mut s2).unwrap();
        let JoinReply::Joined { id: eid, .. } = reply else { panic!() };
        apply_join(&mut shared.lock_world(), eid);
        shared.connected.store(true, Ordering::SeqCst);
        shared.socket_open.store(true, Ordering::SeqCst);
        let sh = Arc::clone(&shared);
        let t = std::thread::spawn(move || send_loop(stream, &sh));
        server.join().unwrap();
        // The server hung up: a send fails and the loop closes the socket.
        t.join().unwrap();
        assert!(!shared.socket_open.load(Ordering::SeqCst));
        assert_eq!(shared.lock_world().local_player, 2);
    }

    #[test]
    fn g6_formatting() {
        assert_eq!(fmt_g6(0.03), "0.03");
        assert_eq!(fmt_g6(3.0 * 0.01), "0.03");
        assert_eq!(fmt_g6(1.0), "1");
        assert_eq!(fmt_g6(123456789.0), "1.23457e+008");
        assert_eq!(fmt_g6(-0.00001), "-1e-005");
    }
}
