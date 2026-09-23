//! Compares two `tools/oracle/client_probe.py` output directories (the original server's and
//! cw-server's): the Join entity block, the first (full) entity update of every entity id, and
//! the ServerUpdate bodies, with the padding bytes of items, entities and statics masked.
//! With `--initial`, the entity fields the tick's physics and AI change before the first send
//! (rotation, velocities, timers, physics flags) are masked too and positions compare with a
//! tolerance of four blocks, so what remains is the block the spawn produced.
//!
//!     cargo run --release -p cw-server --example probe_diff -- [--initial] DIR_A DIR_B

use std::collections::BTreeSet;
use std::path::Path;

use cw_net::{ByteBuffer, ENTITY_SIZE, EntityData, ServerUpdate, read_delta};

/// Bytes of an item the original never writes (structure padding, heap garbage there).
const ITEM_PADDING: [usize; 5] = [2, 3, 0xf, 0x12, 0x13];

fn read(p: &Path) -> Option<Vec<u8>> {
    std::fs::read(p).ok()
}

/// Padding bytes inside an entity block: the thirteen equipment items at 0x2f0, the byte
/// after the appearance's flags (+5), and the gaps after the u16s at 0x114, 0x17c and 0x1e8
/// that `EntityData::EntityData` never writes (0x1da, 0x1db and 0x1e7 likewise). With `initial`, also everything the tick's
/// physics and AI touch before the first send: rotation, velocities, look pitch and physics
/// flags (0x18..0x50), mode and timers (0x58..0x68, 0x114..0x130), the ray hit (0x150..0x15c), MP and block power
/// (0x160..0x168) and the word at 0x1a8.
fn entity_mask(initial: bool) -> Vec<bool> {
    let mut m = vec![false; ENTITY_SIZE];
    for k in 0..13 {
        for o in ITEM_PADDING {
            m[0x2f0 + k * 0x118 + o] = true;
        }
    }
    // 0x51..0x54: the three bytes after the hostile byte, uninitialised in the original's join
    // block (0x50 seen there in one capture).
    for o in [0x51, 0x52, 0x53, 0x6d, 0x116, 0x117, 0x132, 0x133, 0x17e, 0x17f, 0x1da, 0x1db, 0x1e7, 0x1ea, 0x1eb] {
        m[o] = true;
    }
    if initial {
        for r in [0x18..0x50, 0x58..0x68, 0x114..0x130, 0x150..0x15c, 0x160..0x168, 0x1a8..0x1ac] {
            for o in r {
                m[o] = true;
            }
        }
    }
    m
}

/// The largest per-axis position difference in blocks between two entity blocks.
fn position_delta(a: &[u8], b: &[u8]) -> f64 {
    (0..3)
        .map(|i| {
            let o = i * 8;
            let pa = i64::from_le_bytes(a[o..o + 8].try_into().unwrap());
            let pb = i64::from_le_bytes(b[o..o + 8].try_into().unwrap());
            (pa - pb).abs() as f64 / 65536.0
        })
        .fold(0.0, f64::max)
}

fn diff_ranges(a: &[u8], b: &[u8], mask: &[bool]) -> Vec<(usize, usize)> {
    let n = a.len().max(b.len());
    let mut out: Vec<(usize, usize)> = Vec::new();
    for i in 0..n {
        let differs = a.get(i) != b.get(i) && !mask.get(i).copied().unwrap_or(false);
        if differs {
            match out.last_mut() {
                Some(r) if r.1 == i => r.1 = i + 1,
                _ => out.push((i, i + 1)),
            }
        }
    }
    out
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" ")
}

fn compare(name: &str, a: &[u8], b: &[u8], mask: &[bool]) -> bool {
    compare_with(name, a, b, mask, 0.0)
}

/// `compare` with the first 24 bytes (a position) allowed to differ by `tolerance` blocks.
fn compare_with(name: &str, a: &[u8], b: &[u8], mask: &[bool], tolerance: f64) -> bool {
    let mut mask = mask.to_vec();
    if tolerance > 0.0 && a.len() >= 24 && b.len() >= 24 {
        let d = position_delta(a, b);
        if d <= tolerance {
            mask.iter_mut().take(24).for_each(|m| *m = true);
            if d > 0.0 {
                println!("{name}: position differs by {d:.3} blocks (within tolerance)");
            }
        }
    }
    let mask = &mask[..];
    let ranges = diff_ranges(a, b, mask);
    if ranges.is_empty() && a.len() == b.len() {
        println!("{name}: identical ({} bytes)", a.len());
        return true;
    }
    println!("{name}: {} bytes vs {} bytes, {} differing range(s)", a.len(), b.len(), ranges.len());
    for (s, e) in ranges.iter().take(40) {
        let e = (*e).min(s + 16);
        println!("  +0x{s:04x}..+0x{e:04x}: {} | {}", hex(a.get(*s..e.min(a.len())).unwrap_or(&[])), hex(b.get(*s..e.min(b.len())).unwrap_or(&[])));
    }
    false
}

fn decode(delta: &[u8]) -> EntityData {
    let mut buf = ByteBuffer::from_vec(delta.to_vec());
    let mut e = EntityData::ZERO;
    read_delta(&mut buf, &mut e);
    e
}

/// Applies the length-prefixed deltas of an `after_<id>.bin` file in order.
fn apply_after(e: &mut EntityData, after: &[u8]) {
    let mut at = 0usize;
    while at + 4 <= after.len() {
        let n = u32::from_le_bytes(after[at..at + 4].try_into().unwrap()) as usize;
        at += 4;
        if at + n > after.len() {
            break;
        }
        let mut buf = ByteBuffer::from_vec(after[at..at + n].to_vec());
        read_delta(&mut buf, e);
        at += n;
    }
}

/// Applies the deltas of an `after_<id>.bin` file up to and including the first one whose
/// mask carries field `bit`; whether one did.
fn apply_until(e: &mut EntityData, after: &[u8], bit: u32) -> bool {
    let mut at = 0usize;
    while at + 4 <= after.len() {
        let n = u32::from_le_bytes(after[at..at + 4].try_into().unwrap()) as usize;
        at += 4;
        if at + n > after.len() || n < 8 {
            break;
        }
        let mask = u64::from_le_bytes(after[at..at + 8].try_into().unwrap());
        let mut buf = ByteBuffer::from_vec(after[at..at + n].to_vec());
        read_delta(&mut buf, e);
        at += n;
        if mask & (1u64 << bit) != 0 {
            return true;
        }
    }
    false
}

/// The thirteen sections of a re-encoded update body, each as its raw bytes (count included).
fn sections(body: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut at = 0usize;
    let fixed = [0x14usize, 0x48, 0x48, 0x18, 0x70, 0x58, 0, 0x10, 0x120, 0x18, 0x18, 0x28, 0x38];
    for (i, size) in fixed.iter().enumerate() {
        if at + 4 > body.len() {
            break;
        }
        let count = u32::from_le_bytes(body[at..at + 4].try_into().unwrap()) as usize;
        let start = at;
        at += 4;
        if i == 6 {
            for _ in 0..count {
                if at + 12 > body.len() {
                    break;
                }
                let n = u32::from_le_bytes(body[at + 8..at + 12].try_into().unwrap()) as usize;
                at += 12 + n * 0x148;
            }
        } else {
            at += count * size;
        }
        let end = at.min(body.len());
        out.push(body[start..end].to_vec());
    }
    out
}

fn entity_ids(dir: &Path) -> BTreeSet<i64> {
    let mut ids = BTreeSet::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if let Some(rest) = name.strip_prefix("entity_").and_then(|r| r.strip_suffix(".bin"))
                && let Ok(id) = rest.parse::<i64>()
            {
                ids.insert(id);
            }
        }
    }
    ids
}

/// The body re-encoded from its parsed form, so both sides carry zero padding; with
/// `initial`, the statics' `+0x44` timers (milliseconds the tick accumulates) are zeroed too.
fn canonical_update(body: &[u8], initial: bool) -> Option<Vec<u8>> {
    let mut u = ServerUpdate::read_body(body)?;
    for z in &mut u.zone_items {
        for it in &mut z.items {
            // Item padding, and the three bytes after the +0x138 byte of a ground item.
            for o in ITEM_PADDING.into_iter().chain([0x139, 0x13a, 0x13b]) {
                it.0[o] = 0;
            }
        }
    }
    if initial {
        for s in &mut u.statics {
            s.f44 = 0;
        }
        // Sound pitches and ground item rotations and countdowns come from the tick's
        // `rand`, whose state the original's AI has moved on.
        for s in &mut u.sounds {
            // The pitch, and the position: it is the position of a creature the original's
            // physics has moved a little.
            s.0[0x10..0x14].fill(0);
            s.0[0..0xc].fill(0);
        }
        // Hit records: the padding after the +0x14 byte and after the +0x46 byte is garbage.
        for h in &mut u.hits {
            h.0[0x15..0x18].fill(0);
            h.0[0x47] = 0;
        }
        for d in &mut u.damage {
            // The dword after a damage record's threat (+0x14) is never written (0x004d5f40).
            d.0[0x14..0x18].fill(0);
        }
        for z in &mut u.zone_items {
            for it in &mut z.items {
                it.0[0x130..0x134].fill(0);
                it.0[0x13c..0x140].fill(0);
            }
        }
    }
    Some(u.body())
}

fn describe_update(body: &[u8]) -> String {
    match ServerUpdate::read_body(body) {
        Some(u) => {
            let mut parts = Vec::new();
            for (n, c) in [
                ("blocks", u.block_actions.len()),
                ("hits", u.hits.len()),
                ("particles", u.particles.len()),
                ("sounds", u.sounds.len()),
                ("shoots", u.shoots.len()),
                ("statics", u.statics.len()),
                ("zone_items", u.zone_items.len()),
                ("items8", u.items_8.len()),
                ("pickups", u.pickups.len()),
                ("kills", u.kills.len()),
                ("damage", u.damage.len()),
                ("passives", u.passives.len()),
                ("missions", u.missions.len()),
            ] {
                if c > 0 {
                    parts.push(format!("{n}={c}"));
                }
            }
            let mut s = parts.join(" ");
            for m in &u.missions {
                s.push_str(&format!("\n      mission cell ({}, {}) words {:?} b40 {} b41 {} tail {:?}", m.cell_x, m.cell_y, m.words, m.b40, m.b41, m.tail));
            }
            for z in &u.zone_items {
                s.push_str(&format!("\n      zone items ({}, {}): {} items", z.zone_x, z.zone_y, z.items.len()));
                for it in &z.items {
                    let b = &it.0;
                    let pos: Vec<String> = (0..3).map(|i| format!("{:.2}", i64::from_le_bytes(b[0x118 + i * 8..0x120 + i * 8].try_into().unwrap()) as f64 / 65536.0)).collect();
                    s.push_str(&format!("\n        item type {} sub {} mod {:#x} rarity {} material {} level {} f134 {} at {}", b[0], b[1], u32::from_le_bytes(b[4..8].try_into().unwrap()), b[0xc], b[0xd], u16::from_le_bytes(b[0x10..0x12].try_into().unwrap()), f32::from_le_bytes(b[0x134..0x138].try_into().unwrap()), pos.join(", ")));
                }
            }
            for k in &u.kills {
                let b = &k.0;
                s.push_str(&format!("\n      kill by {} of {} type {} xp {}", i64::from_le_bytes(b[0..8].try_into().unwrap()), i64::from_le_bytes(b[8..16].try_into().unwrap()), u32::from_le_bytes(b[16..20].try_into().unwrap()), i32::from_le_bytes(b[20..24].try_into().unwrap())));
            }
            for p in &u.pickups {
                let b = &p.0;
                s.push_str(&format!("\n      pickup for {}: item type {} sub {} rarity {} material {} level {}", i64::from_le_bytes(b[0..8].try_into().unwrap()), b[8], b[9], b[8 + 0xc], b[8 + 0xd], u16::from_le_bytes(b[8 + 0x10..8 + 0x12].try_into().unwrap())));
            }
            for d in &u.damage {
                let b = &d.0;
                s.push_str(&format!("\n      damage: target {} attacker {} threat {}", i64::from_le_bytes(b[0..8].try_into().unwrap()), i64::from_le_bytes(b[8..16].try_into().unwrap()), f32::from_le_bytes(b[16..20].try_into().unwrap())));
            }
            for h in &u.hits {
                let b = &h.0;
                let pos: Vec<String> = (0..3).map(|i| format!("{:.2}", i64::from_le_bytes(b[0x20 + i * 8..0x28 + i * 8].try_into().unwrap()) as f64 / 65536.0)).collect();
                let dir: Vec<String> = (0..3).map(|i| format!("{:.3}", f32::from_le_bytes(b[0x38 + i * 4..0x3c + i * 4].try_into().unwrap()))).collect();
                s.push_str(&format!("
      hit by {} on {}: damage {} crit {} stun {} f1c {} at {} dir {} tail {:02x?}", i64::from_le_bytes(b[0..8].try_into().unwrap()), i64::from_le_bytes(b[8..16].try_into().unwrap()), f32::from_le_bytes(b[0x10..0x14].try_into().unwrap()), b[0x14], u32::from_le_bytes(b[0x18..0x1c].try_into().unwrap()), u32::from_le_bytes(b[0x1c..0x20].try_into().unwrap()), pos.join(", "), dir.join(", "), &b[0x44..0x48]));
            }
            for snd in &u.sounds {
                let b = &snd.0;
                s.push_str(&format!("\n      sound kind {} at {:.1}, {:.1}, {:.1}", u32::from_le_bytes(b[0xc..0x10].try_into().unwrap()), f32::from_le_bytes(b[0..4].try_into().unwrap()), f32::from_le_bytes(b[4..8].try_into().unwrap()), f32::from_le_bytes(b[8..12].try_into().unwrap())));
            }
            s
        }
        None => "unparseable".into(),
    }
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let initial = args.first().is_some_and(|a| a == "--initial");
    if initial {
        args.remove(0);
    }
    if args.len() != 2 {
        eprintln!("usage: probe_diff [--initial] DIR_A DIR_B");
        std::process::exit(2);
    }
    let (a, b) = (Path::new(&args[0]), Path::new(&args[1]));
    let mask = entity_mask(initial);
    let tolerance = if initial { 4.0 } else { 0.0 };
    let mut ok = true;
    if let (Some(ja), Some(jb)) = (read(&a.join("join.bin")), read(&b.join("join.bin"))) {
        ok &= compare("join.bin", &ja, &jb, &mask);
    }
    let (ia, ib) = (entity_ids(a), entity_ids(b));
    for id in ia.union(&ib) {
        match (ia.contains(id), ib.contains(id)) {
            (true, true) => {
                let da = read(&a.join(format!("entity_{id}.bin"))).unwrap_or_default();
                let db = read(&b.join(format!("entity_{id}.bin"))).unwrap_or_default();
                let (ea, eb) = (decode(&da), decode(&db));
                ok &= compare_with(&format!("entity {id} (delta {} vs {} bytes)", da.len(), db.len()), &ea.0, &eb.0, &mask, tolerance);
            }
            (true, false) => {
                println!("entity {id}: only in {}", a.display());
                ok = false;
            }
            (false, true) => {
                println!("entity {id}: only in {}", b.display());
                ok = false;
            }
            _ => {}
        }
    }
    // The state of each creature after the requests: its first delta with every later one
    // applied (`after_<id>.bin`), compared on the fields combat writes. Stun and slowed
    // (0x11c, 0x120) are left out: the original's creature update counts them down by dt
    // without a floor, which is not ported.
    for id in ia.intersection(&ib) {
        let (fa, fb) = (read(&a.join(format!("after_{id}.bin"))), read(&b.join(format!("after_{id}.bin"))));
        if fa.is_none() && fb.is_none() {
            continue;
        }
        let base_a = decode(&read(&a.join(format!("entity_{id}.bin"))).unwrap_or_default());
        let base_b = decode(&read(&b.join(format!("entity_{id}.bin"))).unwrap_or_default());
        let (fa, fb) = (fa.unwrap_or_default(), fb.unwrap_or_default());
        let mut same = true;
        // HP and MP as the first delta carrying them reports them: the original's creature
        // update regenerates both afterwards, which is not ported. MP within 0.01, since a
        // tick of that regeneration may precede the send.
        for (name, bit, o, tol) in [("hp", 27u32, 0x15cusize, 0.0f32), ("mp", 28, 0x160, 0.01)] {
            let (mut ea, mut eb) = (base_a.clone(), base_b.clone());
            let (ha, hb) = (apply_until(&mut ea, &fa, bit), apply_until(&mut eb, &fb, bit));
            let va = f32::from_le_bytes(ea.0[o..o + 4].try_into().unwrap());
            let vb = f32::from_le_bytes(eb.0[o..o + 4].try_into().unwrap());
            if ha != hb || (va - vb).abs() > tol {
                same = false;
                println!("entity {id} after: {name} differs: {va} (sent {ha}) | {vb} (sent {hb})");
            }
        }
        let (mut ea, mut eb) = (base_a.clone(), base_b.clone());
        apply_after(&mut ea, &fa);
        apply_after(&mut eb, &fb);
        for (name, o, n) in [("mode", 0x58usize, 1usize), ("level", 0x180, 4), ("xp", 0x184, 4)] {
            if ea.0[o..o + n] != eb.0[o..o + n] {
                same = false;
                println!("entity {id} after: {name} differs: {:02x?} | {:02x?}", &ea.0[o..o + n], &eb.0[o..o + n]);
            }
        }
        let hp = f32::from_le_bytes(ea.0[0x15c..0x160].try_into().unwrap());
        if same {
            println!("entity {id} after: combat fields identical (final hp {hp})");
        }
        ok &= same;
        // The resting state the physics leaves: final position (within half a block), the
        // physics flags and the velocity, reported for every creature that moved.
        let pd = position_delta(&ea.0, &eb.0);
        let (fa_, fb_) = (u32::from_le_bytes(ea.0[0x4c..0x50].try_into().unwrap()), u32::from_le_bytes(eb.0[0x4c..0x50].try_into().unwrap()));
        let vel = |e: &EntityData| (0..3).map(|i| f32::from_le_bytes(e.0[0x24 + i * 4..0x28 + i * 4].try_into().unwrap())).collect::<Vec<f32>>();
        let z = |e: &EntityData| i64::from_le_bytes(e.0[16..24].try_into().unwrap()) as f64 / 65536.0;
        if pd > 0.5 || fa_ != fb_ {
            println!("entity {id} physics: position differs by {pd:.3} blocks (z {:.3} | {:.3}), flags {fa_:#x} | {fb_:#x}, velocity {:?} | {:?}", z(&ea), z(&eb), vel(&ea), vel(&eb));
            ok = false;
        }
    }
    for n in 1..100 {
        let (ua, ub) = (read(&a.join(format!("update_{n}.bin"))), read(&b.join(format!("update_{n}.bin"))));
        match (ua, ub) {
            (Some(ua), Some(ub)) => {
                println!("update_{n}: A {}\n           B {}", describe_update(&ua), describe_update(&ub));
                let (ca, cb) = (canonical_update(&ua, initial).unwrap_or(ua), canonical_update(&ub, initial).unwrap_or(ub));
                if ca == cb {
                    println!("update_{n}.bin (re-encoded): identical ({} bytes)", ca.len());
                } else {
                    // Section by section, so one extra record does not shift everything after it.
                    let (sa, sb) = (sections(&ca), sections(&cb));
                    for (i, name) in ["block actions", "hits", "particles", "sounds", "shoots", "statics", "zone items", "items8", "pickups", "kills", "damage", "passives", "missions"].iter().enumerate() {
                        let (xa, xb) = (sa.get(i).cloned().unwrap_or_default(), sb.get(i).cloned().unwrap_or_default());
                        if xa != xb {
                            ok &= compare(&format!("update_{n}.bin section {name}"), &xa, &xb, &[]);
                        }
                    }
                }
            }
            (Some(ua), None) => {
                println!("update_{n}: only in A: {}", describe_update(&ua));
                ok = false;
            }
            (None, Some(ub)) => {
                println!("update_{n}: only in B: {}", describe_update(&ub));
                ok = false;
            }
            (None, None) => break,
        }
    }
    println!("{}", if ok { "MATCH" } else { "DIFFERENCES" });
}
