//! Per-connection work: the handshake (`acceptLoop` 0x004254a0), the receive thread
//! (`receiveLoop` 0x00426020) and the send thread (`sendLoop` 0x00423dd0), note sections 2.2,
//! 2.1 and 3.2.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cw_net::packet::{PROTOCOL_VERSION, id};
use cw_net::{ByteBuffer, ClientPacket, EntityData, Parse, ServerPacket, ServerUpdate, parse_client_packet, read_delta, write_delta};

use crate::player::new_player_entity;
use crate::server::{Conn, MAX_PLAYERS, Pending, Server};

fn send_all(stream: &Mutex<TcpStream>, bytes: &[u8]) -> bool {
    stream.lock().unwrap().write_all(bytes).is_ok()
}

fn read_exact(stream: &mut TcpStream, n: usize) -> Option<Vec<u8>> {
    let mut b = vec![0u8; n];
    stream.read_exact(&mut b).ok().map(|_| b)
}

/// `acceptLoop`: one connection at a time, handshake, then the two threads.
pub fn accept_loop(server: Arc<Server>, listener: TcpListener) {
    loop {
        std::thread::sleep(Duration::from_millis(200));
        println!("Waiting for connection.");
        let Ok((mut stream, _)) = listener.accept() else { continue };
        let _ = stream.set_nodelay(false);
        // First packet must be the client version.
        let Some(head) = read_exact(&mut stream, 4) else { continue };
        if u32::from_le_bytes(head.try_into().unwrap()) != id::CLIENT_VERSION {
            continue;
        }
        let Some(v) = read_exact(&mut stream, 4) else { continue };
        let version = u32::from_le_bytes(v.try_into().unwrap());
        println!("Client version: {version}");
        if version != PROTOCOL_VERSION {
            let _ = stream.write_all(&ServerPacket::VersionMismatch.to_bytes());
            std::thread::sleep(Duration::from_millis(500));
            println!("Wrong client version. Closing client connection.");
            continue;
        }
        // The Join id goes out before the entity is created.
        if stream.write_all(&id::JOIN.to_le_bytes()).is_err() {
            continue;
        }
        println!("New connection.");
        let entity_id = {
            let entities = server.entities.lock().unwrap();
            (1..=MAX_PLAYERS).find(|i| !entities.contains_key(i))
        };
        let Some(entity_id) = entity_id else {
            // No free id: the original closes after the bare 0x10.
            continue;
        };
        let entity = {
            let mut world = server.world.lock().unwrap();
            let mut rng = server.accept_rng.lock().unwrap();
            let e = new_player_entity(&mut world, &mut rng);
            server.entities.lock().unwrap().insert(entity_id, e.clone());
            e
        };
        let conn = Arc::new(Conn { id: entity_id, stream: Mutex::new(stream.try_clone().expect("socket clone")), pending: Mutex::new(Pending::default()), alive: AtomicBool::new(true), updates_seen: std::sync::atomic::AtomicU64::new(0) });
        let slot = {
            let mut slots = server.slots.lock().unwrap();
            match slots.iter().position(Option::is_none) {
                Some(i) => {
                    slots[i] = Some(Arc::clone(&conn));
                    i
                }
                None => {
                    slots.push(Some(Arc::clone(&conn)));
                    slots.len() - 1
                }
            }
        };
        println!("Player {slot} joined.");
        // The rest of Join, then Seed.
        let mut bytes = ServerPacket::Join { id: entity_id, entity: Box::new(entity) }.to_bytes();
        bytes.drain(..4);
        bytes.extend_from_slice(&ServerPacket::Seed(server.seed as u32).to_bytes());
        if !send_all(&conn.stream, &bytes) {
            drop_connection(&server, &conn, slot);
            continue;
        }
        let (s1, c1) = (Arc::clone(&server), Arc::clone(&conn));
        let recv = std::thread::Builder::new().name(format!("recv-{entity_id}")).spawn(move || receive_loop(s1, c1, stream)).expect("receive thread");
        let (s2, c2) = (Arc::clone(&server), Arc::clone(&conn));
        std::thread::Builder::new()
            .name(format!("send-{entity_id}"))
            .spawn(move || {
                send_loop(&s2, &c2);
                c2.alive.store(false, Ordering::Relaxed);
                let _ = c2.stream.lock().unwrap().shutdown(std::net::Shutdown::Both);
                let _ = recv.join();
                drop_connection(&s2, &c2, slot);
            })
            .expect("send thread");
    }
}

/// The end of sendLoop: close the socket, delete the creature, free the slot. Nothing is sent
/// to the other clients.
fn drop_connection(server: &Server, conn: &Conn, slot: usize) {
    server.entities.lock().unwrap().remove(&conn.id);
    let mut slots = server.slots.lock().unwrap();
    if let Some(s) = slots.get_mut(slot) {
        *s = None;
    }
    println!("Player {slot} left.");
}

/// `receiveLoop`: framing per note 2.1. A `recv` of 0 is treated as a disconnect (the
/// original does not, and keeps looping on stale bytes).
fn receive_loop(server: Arc<Server>, conn: Arc<Conn>, mut stream: TcpStream) {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    while conn.alive.load(Ordering::Relaxed) {
        let n = match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        buf.extend_from_slice(&chunk[..n]);
        loop {
            match parse_client_packet(&buf) {
                Parse::NeedMore => break,
                Parse::Fatal => {
                    conn.alive.store(false, Ordering::Relaxed);
                    return;
                }
                Parse::Packet(p, used) => {
                    buf.drain(..used);
                    handle(&server, &conn, p);
                }
            }
        }
    }
    conn.alive.store(false, Ordering::Relaxed);
}

fn handle(server: &Server, conn: &Conn, p: ClientPacket) {
    if server.verbose {
        match &p {
            ClientPacket::EntityUpdate { compressed } => {
                let n = conn.updates_seen.fetch_add(1, Ordering::Relaxed);
                if n.is_multiple_of(100) {
                    println!("[{}] entity update #{n} ({} bytes compressed)", conn.id, compressed.len());
                }
            }
            ClientPacket::Hit(h) => println!("[{}] hit: {:02x?}", conn.id, &h.0[..0x20]),
            ClientPacket::Passive(p) => println!("[{}] passive: {:02x?}", conn.id, &p.0[..0x10]),
            ClientPacket::Shoot(s) => println!("[{}] shoot: {:02x?}", conn.id, &s.0[..0x20]),
            ClientPacket::Chat(t) => println!("[{}] chat: {}", conn.id, String::from_utf16_lossy(t)),
            ClientPacket::ZoneDiscovered { x, y } => println!("[{}] zone discovered {x},{y}", conn.id),
            ClientPacket::RegionDiscovered { x, y } => println!("[{}] region discovered {x},{y}", conn.id),
            ClientPacket::Interact(i) => {
                let w = |o: usize| i32::from_le_bytes(i.0[o..o + 4].try_into().unwrap());
                println!("[{}] interact type {} at zone {},{} index {} (+0x124 {}, item {}/{}, tail {:02x?})", conn.id, i.0[0x128], w(0x118), w(0x11c), w(0x120), w(0x124), i.0[0], i.0[1], &i.0[0x129..0x12c]);
            }
            ClientPacket::Version(v) => println!("[{}] version {v}", conn.id),
            ClientPacket::Ignored(id) => println!("[{}] packet id {id} (no payload)", conn.id),
        }
    }
    match p {
        ClientPacket::EntityUpdate { compressed } => {
            // The id in the payload is ignored: the update applies to this connection's creature.
            if let Some((_, delta)) = ClientPacket::decode_entity_update(&compressed) {
                let mut entities = server.entities.lock().unwrap();
                if let Some(e) = entities.get_mut(&conn.id) {
                    let mut tmp = e.clone();
                    drop(entities);
                    let mut r = ByteBuffer::from_vec(delta);
                    read_delta(&mut r, &mut tmp);
                    let mut entities = server.entities.lock().unwrap();
                    if let Some(e) = entities.get_mut(&conn.id) {
                        *e = tmp;
                    }
                }
            }
        }
        ClientPacket::Hit(h) => server.inbox.lock().unwrap().0.push(*h),
        ClientPacket::Passive(p) => server.inbox.lock().unwrap().1.push(*p),
        ClientPacket::Shoot(s) => server.inbox.lock().unwrap().2.push(*s),
        ClientPacket::Chat(text) => server.broadcast_chat(conn.id, text),
        ClientPacket::ZoneDiscovered { x, y } => conn.pending.lock().unwrap().zones.push((x, y)),
        ClientPacket::RegionDiscovered { x, y } => conn.pending.lock().unwrap().regions.push((x, y)),
        ClientPacket::Interact(i) => server.interacts.lock().unwrap().push((conn.id, *i)),
        ClientPacket::Version(_) | ClientPacket::Ignored(_) => {}
    }
}

/// The squared distance in blocks from `me` to `e` as sendLoop computes it: each axis is the
/// 64-bit difference rounded once to f32, times 2^-16; the squares add as `dy + dx + dz`.
fn creature_dist_sq(me: &EntityData, e: &EntityData) -> f32 {
    const K: f32 = 1.5258789e-05;
    let d = |o: usize| i64::from_le_bytes(e.0[o..o + 8].try_into().unwrap()).wrapping_sub(i64::from_le_bytes(me.0[o..o + 8].try_into().unwrap())) as f32 * K;
    let (dx, dy, dz) = (d(0), d(8), d(16));
    (dy * dy + dx * dx) + dz * dz
}

/// `sendLoop`, one frame per 20 ms (paced per thread, not through the original's shared
/// timestamp): entity deltas in ascending id order (at most one entity new to this client per
/// frame, none in the first 500 ms), UpdateFinished, CurrentTime, ServerUpdate, one chat line.
fn send_loop(server: &Server, conn: &Conn) {
    let start = Instant::now();
    let mut sent: BTreeMap<i64, EntityData> = BTreeMap::new();
    let mut last = Instant::now();
    while conn.alive.load(Ordering::Relaxed) && server.running.load(Ordering::Relaxed) {
        let mut allow_new = start.elapsed() > Duration::from_millis(500);
        let mut frame: BTreeMap<i64, EntityData> = BTreeMap::new();
        {
            let entities = server.entities.lock().unwrap();
            let me = entities.get(&conn.id).cloned();
            for (id, e) in entities.iter() {
                if *id == conn.id {
                    continue;
                }
                // Creatures (hostile type != 0) farther than 200 blocks are skipped; players
                // never are (0x00423f5c..0x0042402a).
                if e.0[0x50] != 0 && let Some(me) = &me && creature_dist_sq(me, e) > 40000.0f32 {
                    continue;
                }
                if sent.contains_key(id) {
                    frame.insert(*id, e.clone());
                } else if allow_new {
                    frame.insert(*id, e.clone());
                    allow_new = false;
                }
            }
            sent.retain(|id, _| entities.contains_key(id));
        }
        let mut failed = false;
        let mut out = Vec::new();
        for (id, e) in &frame {
            let mut buf = ByteBuffer::new();
            match sent.get(id) {
                Some(prev) => write_delta(&mut buf, prev, e, false),
                None => write_delta(&mut buf, e, e, true),
            };
            ServerPacket::EntityUpdate { id: *id, delta: buf.into_vec() }.encode(&mut out);
        }
        ServerPacket::UpdateFinished.encode(&mut out);
        if !send_all(&conn.stream, &out) {
            failed = true;
        }
        sent = frame;
        let (day, time, update, chat) = {
            let world = server.world.lock().unwrap();
            let mut p = conn.pending.lock().unwrap();
            (world.day as u32, world.time_of_day as u32, std::mem::take(&mut p.update), p.chat.pop_front())
        };
        let mut out = Vec::new();
        ServerPacket::CurrentTime { day, time }.encode(&mut out);
        if !failed {
            ServerPacket::ServerUpdate(Box::new(update)).encode(&mut out);
        }
        if let Some((sender, text)) = chat
            && !text.is_empty()
        {
            ServerPacket::Chat { sender, text }.encode(&mut out);
        }
        if failed || !send_all(&conn.stream, &out) {
            break;
        }
        let elapsed = last.elapsed();
        if elapsed < Duration::from_millis(20) {
            std::thread::sleep(Duration::from_millis(20) - elapsed);
        }
        last = Instant::now();
    }
    let _ = ServerUpdate::default();
}
