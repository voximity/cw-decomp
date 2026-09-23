r"""In-process oracle for Server.exe (Cube World Alpha, 2013-07-23 build).

Spawns game/Server.exe under Frida, captures the cube::World instance, and exposes the
original binary's functions so Rust tests can be checked against the real thing.

    .venv\Scripts\python.exe tools/oracle/server_oracle.py noise --count 20000 --out crates/cw-math/tests/golden/noise2d.txt
    .venv\Scripts\python.exe tools/oracle/server_oracle.py rand --seed 26879 --count 64

Server.exe listens on TCP 12345 while this runs; it is killed on exit.
"""
import argparse
import hashlib
import random
import struct
import sys
import time
from pathlib import Path

import frida

ROOT = Path(__file__).resolve().parent.parent.parent
GAME = ROOT / "game"
IMAGE_BASE = 0x00400000

# Addresses in Server.exe (see analysis/names/Server.exe.json).
ADDR = {
    "World::World": 0x004C8570,
    "valueNoise2D": 0x004D5D30,
    "World::generateZone": 0x00518630,
    "World::getZone": 0x00406290,
    "World::lock": 0x004D3DF0,
    "World::getOrCreateClimatePoint": 0x0050B870,
    "World::createRegion": 0x0050E080,
    "World::getRegion": 0x00406210,
    "World::unlock": 0x004D5C60,
    "Zone::Zone": 0x00548B60,
}

AGENT_JS = r"""
const base = Process.getModuleByName('Server.exe').base;
const rva = a => base.add(a - %(image_base)d);
let world = null;
Interceptor.attach(rva(%(world_ctor)d), {
  onEnter(args) { world = this.context.ecx; send({ type: 'world', ptr: world.toString() }); }
});
const noise = new NativeFunction(rva(%(noise)d), 'float', ['double', 'double'], 'mscdecl');
const msvcr = Process.getModuleByName('MSVCR110.dll');
const c_rand = new NativeFunction(msvcr.getExportByName('rand'), 'int', [], 'mscdecl');
const c_srand = new NativeFunction(msvcr.getExportByName('srand'), 'void', ['uint'], 'mscdecl');

const genZone = new NativeFunction(rva(%(gen_zone)d), 'void', ['pointer', 'int', 'int'], 'thiscall');
const getZone = new NativeFunction(rva(%(get_zone)d), 'pointer', ['pointer', 'int', 'int'], 'thiscall');
const getPoint = new NativeFunction(rva(%(get_point)d), 'pointer', ['pointer', 'int', 'int'], 'thiscall');
const createRegion = new NativeFunction(rva(%(create_region)d), 'void', ['pointer', 'int', 'int'], 'thiscall');
const getRegion = new NativeFunction(rva(%(get_region)d), 'pointer', ['pointer', 'int', 'int'], 'thiscall');
const lockWorld = new NativeFunction(rva(%(lock)d), 'void', ['pointer'], 'thiscall');
const unlockWorld = new NativeFunction(rva(%(unlock)d), 'void', ['pointer'], 'thiscall');

// Serialize one zone (format version 5): header, then 256*256 columns in x + y*256 order, then
// the spawn, ground item, static, prop and marker records (each u32-count then u32-length-prefixed
// records; only the fields the generator writes, since the constructors leave padding uninitialised).
// Column record: f32 climateA, f32 climateB, f32 climateC, i32 base, i32 field14, u32 nblocks, then nblocks*4 bytes.
// Item projection (275 bytes): +0 2 bytes, +4 4, +8 4, +0xc 3, +0x10 2, +0x14 256 spirit bytes, +0x114 4.
// Inventory (0x130 bytes at Spawn+0xf6c / Static+0x48): i32 gold (+0x128), i32 +0x12c, u32 pages, per page
//   u32 slots, per slot i32 count + item projection.
// Spawn (cube::Spawn*, vector at zone+0x18): +0x10..+0x28 position (3 x i64), +0x28 i32, +0x2c entity type,
//   +0x30 u16, +0x34 level, +0x38..+0x50 (6 x u32), +0x50 u8, +0x54 rotation f32, +0x58 u8, +0x7a appearance
//   flags u16, +0xf58 f32 (74 bytes), then 13 equipment items (+0x120 + k*0x118), +0xf5c..+0xf6c (4 f32),
//   the inventory, the int vectors at +0x10ac and +0x10bc (u32 n + ints) around +0x10b8 i32, and +0x10e8 u8.
// Ground item (0x148 bytes, vector at zone+0x30): item projection, +0x118 position (3 x i64), +0x130 f32,
//   +0x134 f32, +0x138 u8, +0x13c i32, +0x140 i32, +0x144 i32.
// Static (0x188 bytes, vector at zone+0xc): +0 u32 type, +8 position (3 x i64), +0x20 i32, +0x24 3 x f32,
//   +0x30 u8, +0x34 u32, +0x38 u32, +0x40 u32, +0x44 u32, +0x54 u32, +0x170..+0x188 (89 bytes), then the
//   inventory at +0x48.
// Prop (std::list at zone+4, value at node+8): +0 u32 kind, +8 position (3 x i64), +0x20 f32, +0x24 f32
//   (both dumped as 0 for the tree props 0x3c/0x3d, which never write them), +0x2c 3 x f32, +0x38 u32 = 52
//   bytes (+0x28 is uninitialised until the end of generateZone and left out).
// Marker (0x140 bytes, vector at zone+0x48): +0 u32, +4 2 bytes, +0x10 3 bytes, +0x14 u16, +0x18 256 bytes,
//   +0x118 u32, +0x11c 2 x i32, +0x128 position (3 x i64) = 303 bytes.
// Then (format 6) the dirty byte zone+0x75 and the modified-block records of the vector at zone+0x68
//   (20 bytes each: i32 x, y, z, the block, i32 day).
function serializeZone(zone) {
  const cols = zone.add(0xa8).readPointer();
  const parts = [];
  let total = 0;
  const push = (buf) => { parts.push(buf); total += buf.byteLength; };
  const u32 = (v) => { const b = new ArrayBuffer(4); new DataView(b).setUint32(0, v, true); return b; };
  const header = new ArrayBuffer(16);
  const hv = new DataView(header);
  hv.setUint32(0, 0x454e4f5a, true);            // 'ZONE'
  hv.setInt32(4, zone.add(0x60).readS32(), true);
  hv.setInt32(8, zone.add(0x64).readS32(), true);
  hv.setUint32(12, 6, true);                    // format version
  push(header);
  for (let i = 0; i < 65536; i++) {
    const c = cols.add(i * 32);
    const n = c.add(0x1c).readS32();
    const buf = new ArrayBuffer(24);
    const dv = new DataView(buf);
    dv.setFloat32(0, c.add(4).readFloat(), true);
    dv.setFloat32(4, c.add(8).readFloat(), true);
    dv.setFloat32(8, c.add(0xc).readFloat(), true);
    dv.setInt32(12, c.add(0x10).readS32(), true);
    dv.setInt32(16, c.add(0x14).readS32(), true);
    dv.setUint32(20, n, true);
    push(buf);
    if (n > 0) push(c.add(0x18).readPointer().readByteArray(n * 4));
  }
  const vec = (off, size) => { const b = zone.add(off).readPointer(); const e = zone.add(off + 4).readPointer(); return [b, e.sub(b).toInt32() / size]; };
  const cat = (chunks) => { let n = 0; for (const c of chunks) n += c.byteLength; const o = new Uint8Array(n); let k = 0; for (const c of chunks) { o.set(new Uint8Array(c), k); k += c.byteLength; } return o.buffer; };
  const fieldsOf = (p, ranges) => cat(ranges.map(([o, n]) => p.add(o).readByteArray(n)));
  const itemOf = (p) => fieldsOf(p, [[0, 2], [4, 4], [8, 4], [0xc, 3], [0x10, 2], [0x14, 0x100], [0x114, 4]]);
  const i32 = (v) => { const b = new ArrayBuffer(4); new DataView(b).setInt32(0, v, true); return b; };
  const inventoryOf = (p) => {
    const chunks = [p.add(0x128).readByteArray(8)];
    const pb = p.readPointer(), pe = p.add(4).readPointer();
    const npages = pe.sub(pb).toInt32() / 12;
    chunks.push(u32(npages));
    for (let i = 0; i < npages; i++) {
      const q = pb.add(i * 12);
      const sb = q.readPointer(), se = q.add(4).readPointer();
      const nslots = se.sub(sb).toInt32() / 0x11c;
      chunks.push(u32(nslots));
      for (let k = 0; k < nslots; k++) { const sl = sb.add(k * 0x11c); chunks.push(sl.readByteArray(4)); chunks.push(itemOf(sl.add(4))); }
    }
    return cat(chunks);
  };
  const intVecOf = (p) => { const b = p.readPointer(), e = p.add(4).readPointer(); const n = e.sub(b).toInt32() / 4; return cat([u32(n), n > 0 ? b.readByteArray(n * 4) : new ArrayBuffer(0)]); };
  const records = (list) => { push(u32(list.length)); for (const r of list) { push(u32(r.byteLength)); push(r); } };
  let [b, n] = vec(0x18, 4);
  const spawns = [];
  for (let i = 0; i < n; i++) {
    const s = b.add(i * 4).readPointer();
    const chunks = [fieldsOf(s, [[0x10, 24], [0x28, 4], [0x2c, 4], [0x30, 2], [0x34, 4], [0x38, 24], [0x50, 1], [0x54, 4], [0x58, 1], [0x7a, 2], [0xf58, 4]])];
    for (let k = 0; k < 13; k++) chunks.push(itemOf(s.add(0x120 + k * 0x118)));
    chunks.push(s.add(0xf5c).readByteArray(16));
    chunks.push(inventoryOf(s.add(0xf6c)));
    chunks.push(intVecOf(s.add(0x10ac)));
    chunks.push(s.add(0x10b8).readByteArray(4));
    chunks.push(intVecOf(s.add(0x10bc)));
    chunks.push(s.add(0x10e8).readByteArray(1));
    spawns.push(cat(chunks));
  }
  records(spawns);
  [b, n] = vec(0x30, 0x148);
  const items = [];
  for (let i = 0; i < n; i++) { const p = b.add(i * 0x148); items.push(cat([itemOf(p), fieldsOf(p, [[0x118, 24], [0x130, 4], [0x134, 4], [0x138, 1], [0x13c, 12]])])); }
  records(items);
  [b, n] = vec(0xc, 0x188);
  const statics = [];
  for (let i = 0; i < n; i++) { const p = b.add(i * 0x188); statics.push(cat([fieldsOf(p, [[0, 4], [8, 24], [0x20, 4], [0x24, 12], [0x30, 1], [0x34, 8], [0x40, 8], [0x54, 4], [0x170, 24]]), inventoryOf(p.add(0x48))])); }
  records(statics);
  const head = zone.add(4).readPointer();
  const props = [];
  for (let node = head.readPointer(); !node.equals(head); node = node.readPointer()) {
    const p = node.add(8);
    const kind = p.readU32();
    if (kind === 0x3c || kind === 0x3d) props.push(cat([fieldsOf(p, [[0, 4], [8, 24]]), new ArrayBuffer(8), fieldsOf(p, [[0x2c, 12], [0x38, 4]])]));
    else props.push(fieldsOf(p, [[0, 4], [8, 24], [0x20, 4], [0x24, 4], [0x2c, 12], [0x38, 4]]));
  }
  records(props);
  [b, n] = vec(0x48, 0x140);
  const markers = [];
  for (let i = 0; i < n; i++) markers.push(fieldsOf(b.add(i * 0x140), [[0, 4], [4, 2], [0x10, 3], [0x14, 2], [0x18, 0x100], [0x118, 4], [0x11c, 8], [0x128, 24]]));
  records(markers);
  push(zone.add(0x75).readByteArray(1));
  [b, n] = vec(0x68, 0x14);
  const mods = [];
  for (let i = 0; i < n; i++) mods.push(b.add(i * 0x14).readByteArray(0x14));
  records(mods);
  const out = new Uint8Array(total);
  let off = 0;
  for (const p of parts) { out.set(new Uint8Array(p), off); off += p.byteLength; }
  return out.buffer;
}

// Generic call. argSpec items: 'world' | 'i32' | 'u32' | 'f32' | 'f64' | 'out_f32x<n>' | 'out_f64x<n>' | 'in_i64' (as [lo, hi]).
// Returns { ret, outs: [[...], ...] }.
const fnCache = {};
function getFn(addr, abi, ret, argSpec) {
  const key = addr + abi + ret + argSpec.join(',');
  if (!fnCache[key]) {
    const types = argSpec.map(a => (a === 'world' || a.startsWith('out_') || a.startsWith('in_')) ? 'pointer' : a);
    fnCache[key] = new NativeFunction(rva(addr), ret, types, abi);
  }
  return fnCache[key];
}
function callFn(addr, abi, ret, argSpec, args) {
  const fn = getFn(addr, abi, ret, argSpec);
  const outs = [];
  const natArgs = argSpec.map((a, i) => {
    if (a === 'world') return world;
    if (a.startsWith('out_f32x')) { const n = parseInt(a.slice(8)); const m = Memory.alloc(n * 4); outs.push([m, 'f32', n]); return m; }
    if (a.startsWith('out_f64x')) { const n = parseInt(a.slice(8)); const m = Memory.alloc(n * 8); outs.push([m, 'f64', n]); return m; }
    if (a.startsWith('out_u32x')) { const n = parseInt(a.slice(8)); const m = Memory.alloc(n * 4); outs.push([m, 'u32', n]); return m; }
    if (a === 'in_i64') { const m = Memory.alloc(8); m.writeU32(args[i][0] >>> 0); m.add(4).writeU32(args[i][1] >>> 0); return m; }
    return args[i];
  });
  const r = fn(...natArgs);
  const outVals = outs.map(([m, t, n]) => { const v = []; for (let k = 0; k < n; k++) v.push(t === 'f32' ? m.add(k * 4).readFloat() : t === 'u32' ? m.add(k * 4).readU32() : m.add(k * 8).readDouble()); return v; });
  return { ret: (r instanceof NativePointer) ? r.toString() : r, outs: outVals };
}

// The save database (cube::Database at world+0xac; sqlite3* at +4) and World::saveZone / saveEntities.
const putBlobVec = new NativeFunction(rva(0x00413210), 'uint8', ['pointer', 'pointer', 'pointer'], 'thiscall');
const getBlobVec = new NativeFunction(rva(0x00413130), 'uint8', ['pointer', 'pointer', 'pointer'], 'thiscall');
const saveZoneFn = new NativeFunction(rva(0x004d81b0), 'void', ['pointer', 'pointer'], 'thiscall');
const saveEntitiesFn = new NativeFunction(rva(0x004d7c50), 'void', ['pointer', 'int', 'int'], 'thiscall');
// An MSVC 2012 std::string in its heap form (capacity >= 16 selects the pointer at +0).
function stdString(s) {
  const buf = Memory.allocUtf8String(s);
  const str = Memory.alloc(0x18);
  str.writePointer(buf); str.add(0x10).writeU32(s.length); str.add(0x14).writeU32(0x1f);
  return [str, buf];
}

// The masked entity delta codec (protocol.md 2.3): writeEntityDelta(ByteBuffer*, prev*, cur*, full) and
// readEntityDelta(ByteBuffer*, target*). ByteBuffer is {begin, end, cap, pos}.
const writeEntityDelta = new NativeFunction(rva(0x00416210), 'int', ['pointer', 'pointer', 'pointer', 'uint8'], 'mscdecl');
const readEntityDelta = new NativeFunction(rva(0x00415dd0), 'int', ['pointer', 'pointer'], 'mscdecl');
const ENTITY_SIZE = 0x1168;
function entityBlock(bytes) {
  // 16 zero bytes follow the block so the name strcmp stops when no NUL is inside the field.
  const p = Memory.alloc(ENTITY_SIZE + 16);
  p.writeByteArray(bytes);
  p.add(ENTITY_SIZE).writeByteArray(new Array(16).fill(0));
  return p;
}

const c_cos = new NativeFunction(msvcr.getExportByName('cos'), 'double', ['double'], 'mscdecl');
const c_sin = new NativeFunction(msvcr.getExportByName('sin'), 'double', ['double'], 'mscdecl');
const PI_F = 3.14159274101257324;  // float pi widened, the constant valueNoise2D multiplies by

// Deterministic input sequences hashed with FNV-1a 64 over the little-endian output bytes, so
// millions of samples fit in one line. The Rust tests regenerate the same inputs.
function fnv1a(hash, buf) {
  const bytes = new Uint8Array(buf);
  for (let i = 0; i < bytes.length; i++) { hash ^= BigInt(bytes[i]); hash = (hash * 0x100000001b3n) & 0xffffffffffffffffn; }
  return hash;
}
function lcg32(i) { return (Math.imul(i, 0x9e3779b1) >>> 0); }

// The zone under construction: cube::Zone::Zone runs once per generateZone.
let currentZone = null;
Interceptor.attach(rva(%(zone_ctor)d), { onEnter(args) { currentZone = this.context.ecx; } });

// Mid-generation snapshots. generateZone is never patched mid-function: each stage hooks the
// entry (or exit) of a callee and fires only when the return address is the given call site in
// generateZone, i.e. the first call of the next pass (or the call being bracketed).
// [callee, return address, 'enter' | 'leave']
const STAGES = {
  terrain:    [0x004f7b60, 0x0051a9b7, 'enter'],  // list ctor after the terrain pass
  features:   [0x0052d990, 0x0051b48e, 'enter'],  // first heightFactorB of the ground-layer pass
  layers:     [0x0052cd50, 0x0051bc6a, 'enter'],  // first heightFactorA of the terrace pass
  dungeons:   [0x004d5d30, 0x0051c44b, 'enter'],  // first noise of the terrace-blob pass (after generateDungeon)
  decor1:     [0x004d5d30, 0x0051c7eb, 'enter'],  // first noise of the mark-blob pass
  trees:      [0x00406100, 0x0051dd4e, 'enter'],  // first getColumn of the tree grid
  creatures:  [0x00406100, 0x0051ee79, 'enter'],  // first getColumn of the creature grid
  decor2:     [0x00406100, 0x0051fa5e, 'enter'],  // first getColumn of the per-column prop pass
  appearance: [0x0040a840, 0x00521114, 'enter'],  // first Creature::initAppearance
  dungeon_in:  [0x00500300, 0x0051c2a8, 'enter'], dungeon_out:  [0x00500300, 0x0051c2a8, 'leave'],
  dungeon2_in: [0x00500300, 0x0051c2dc, 'enter'], dungeon2_out: [0x00500300, 0x0051c2dc, 'leave'],
  model_in:    [0x00524540, 0x0051cb60, 'enter'], model_out:    [0x00524540, 0x0051cb60, 'leave'],
  settle_in:   [0x004e28e0, 0x0051d457, 'enter'], settle_out:   [0x004e28e0, 0x0051d457, 'leave'],
  populate_in: [0x005104e0, 0x0051eacc, 'enter'], populate_out: [0x005104e0, 0x0051eacc, 'leave'],
};
let stageSnapshots = null;
function attachStages(names) {
  const byCallee = {};
  for (const n of names) {
    if (n === 'full') continue;
    const s = STAGES[n];
    if (!s) throw new Error('unknown stage ' + n);
    (byCallee[s[0]] = byCallee[s[0]] || []).push([n, rva(s[1]), s[2]]);
  }
  const listeners = [];
  for (const callee in byCallee) {
    const wanted = byCallee[callee];
    listeners.push(Interceptor.attach(rva(parseInt(callee)), {
      onEnter(args) {
        this.leaveAs = null;
        for (const [n, ret, when] of wanted) {
          if (!this.returnAddress.equals(ret)) continue;
          try {
            if (when === 'enter') { if (!(n in stageSnapshots)) stageSnapshots[n] = serializeZone(currentZone); }
            else this.leaveAs = n;
          } catch (e) { send({ type: 'error', message: 'stage ' + n + ': ' + e }); }
        }
      },
      onLeave(ret) {
        if (this.leaveAs && !(this.leaveAs in stageSnapshots)) {
          try { stageSnapshots[this.leaveAs] = serializeZone(currentZone); }
          catch (e) { send({ type: 'error', message: 'stage ' + this.leaveAs + ': ' + e }); }
        }
      }
    }));
  }
  return listeners;
}

rpc.exports = {
  world() { return world ? world.toString() : null; },
  // Generates the zone and returns { stage: snapshot } for the requested STAGES names; 'full' is the
  // finished zone. A stage that never fires (its pass was skipped for this zone) is left out.
  generateZoneStages(zx, zy, names) {
    lockWorld(world);
    const listeners = attachStages(names);
    stageSnapshots = {};
    try {
      genZone(world, zx, zy);
      if (names.indexOf('full') >= 0) {
        const z = getZone(world, zx, zy);
        if (!z.isNull()) stageSnapshots.full = serializeZone(z);
      }
      return Object.keys(stageSnapshots);
    } finally {
      for (const l of listeners) l.detach();
      unlockWorld(world);
    }
  },
  // Frida only transfers a top-level ArrayBuffer, so snapshots are fetched one at a time.
  takeSnapshot(name) { const s = stageSnapshots[name]; delete stageSnapshots[name]; return s; },
  // Creates the regions in order and returns, per region: [level, f10, variant, cells[64], zoneRecords[]].
  // cell = [xlo, xhi, ylo, yhi, radiusBits, heightBits, type, variant, id, level, extra]; zone record =
  // [index, kind, sub, seed, level, byte0c] for records that differ from the constructor's defaults.
  createRegions(list) {
    return list.map(([rx, ry]) => {
      lockWorld(world);
      try {
        createRegion(world, rx, ry);
        const r = getRegion(world, rx, ry);
        if (r.isNull()) return null;
        const cells = [];
        for (let i = 0; i < 64; i++) {
          const c = r.add(0x14018 + i * 0x68);
          cells.push([c.readU32(), c.add(4).readS32(), c.add(8).readU32(), c.add(12).readS32(), c.add(16).readU32(), c.add(20).readU32(),
                      c.add(24).readS32(), c.add(28).readS32(), c.add(32).readS32(), c.add(36).readS32(), c.add(40).readS32()]);
        }
        const recs = [];
        for (let i = 0; i < 4096; i++) {
          const z = r.add(0x18 + i * 16);
          const kind = z.readU8(), sub = z.add(1).readU8(), seed = z.add(4).readS32(), level = z.add(8).readS32(), b0c = z.add(12).readU8();
          if (kind !== 0 || sub !== 0 || seed !== 0 || level !== 1 || b0c !== 0) recs.push([i, kind, sub, seed, level, b0c]);
        }
        return [r.add(0xc).readS32(), r.add(0x10).readS32(), r.add(0x14).readS32(), cells, recs];
      } finally { unlockWorld(world); }
    });
  },
  cosBatch(xs) { return xs.map(x => c_cos(x)); },
  sinBatch(xs) { return xs.map(x => c_sin(x)); },
  // sin over [0, 2*PI_F): x_i = (lcg32(i) / 2^32) * 2 * PI_F
  sinHash(count) {
    let h = 0xcbf29ce484222325n;
    const buf = new ArrayBuffer(8); const dv = new DataView(buf);
    for (let i = 0; i < count; i++) { const x = (lcg32(i) / 4294967296) * 2 * PI_F; dv.setFloat64(0, c_sin(x), true); h = fnv1a(h, buf); }
    return h.toString(16);
  },
  // cos over [0, PI_F): x_i = (lcg32(i) / 2^32) * PI_F
  cosHash(count) {
    let h = 0xcbf29ce484222325n;
    const buf = new ArrayBuffer(8); const dv = new DataView(buf);
    for (let i = 0; i < count; i++) { const x = (lcg32(i) / 4294967296) * PI_F; dv.setFloat64(0, c_cos(x), true); h = fnv1a(h, buf); }
    return h.toString(16);
  },
  // valueNoise2D over x in [-1e5, 1.1e6), y likewise: x_i = lcg32(2i)/2^32 * 1.2e6 - 1e5
  noiseHash(count) {
    let h = 0xcbf29ce484222325n;
    const buf = new ArrayBuffer(4); const dv = new DataView(buf);
    for (let i = 0; i < count; i++) {
      const x = (lcg32(2 * i) / 4294967296) * 1.2e6 - 1e5;
      const y = (lcg32(2 * i + 1) / 4294967296) * 1.2e6 - 1e5;
      dv.setFloat32(0, noise(x, y), true); h = fnv1a(h, buf);
    }
    return h.toString(16);
  },
  // Creates the climate points for the given regions (idempotent) and returns each point's fields:
  // [x, y, flag, A (f32 bits), B (f32 bits), seed, elevation].
  climatePoints(regions) {
    return regions.map(([rx, ry]) => {
      const p = getPoint(world, rx, ry);
      if (p.isNull()) return null;
      return [p.readS32(), p.add(4).readS32(), p.add(8).readU8(), p.add(12).readU32(), p.add(16).readU32(), p.add(20).readS32(), p.add(24).readS32()];
    });
  },
  callBatch(addr, abi, ret, argSpec, argsList) { return argsList.map(args => callFn(addr, abi, ret, argSpec, args)); },
  // writeEntityDelta over two 0x1168-byte blocks: the buffer contents (mask + changed fields).
  entityDelta(prev, cur, full) {
    const p = entityBlock(prev), c = entityBlock(cur);
    const buf = Memory.alloc(16);
    buf.writePointer(NULL); buf.add(4).writePointer(NULL); buf.add(8).writePointer(NULL); buf.add(12).writeU32(0);
    writeEntityDelta(buf, p, c, full ? 1 : 0);
    const b = buf.readPointer(), e = buf.add(4).readPointer();
    const n = e.sub(b).toInt32();
    return n > 0 ? Array.from(new Uint8Array(b.readByteArray(n))) : [];
  },
  // readEntityDelta of `delta` into `target`: the block afterwards and the cursor position.
  entityRead(delta, target) {
    const n = delta.length;
    const d = Memory.alloc(Math.max(n, 1));
    if (n > 0) d.writeByteArray(delta);
    const buf = Memory.alloc(16);
    buf.writePointer(d); buf.add(4).writePointer(d.add(n)); buf.add(8).writePointer(d.add(n)); buf.add(12).writeU32(0);
    const t = entityBlock(target);
    readEntityDelta(buf, t);
    return { bytes: Array.from(new Uint8Array(t.readByteArray(ENTITY_SIZE))), pos: buf.add(12).readS32() };
  },
  // Save database access (only meaningful when game/Save existed at start-up, so World::load opened the file).
  dbOpen() { return !world.add(0xb0).readPointer().isNull(); },
  putBlob(key, bytes) {
    const n = bytes.length;
    const data = Memory.alloc(Math.max(n, 1));
    if (n > 0) data.writeByteArray(bytes);
    const vec = Memory.alloc(12);
    vec.writePointer(data); vec.add(4).writePointer(data.add(n)); vec.add(8).writePointer(data.add(n));
    const [str, buf] = stdString(key);
    lockWorld(world);
    try { return putBlobVec(world.add(0xac), str, vec) !== 0 && buf !== null; } finally { unlockWorld(world); }
  },
  getBlob(key) {
    const vec = Memory.alloc(16);  // begin, end, capacity end, and the position word getBlobVec clears
    vec.writePointer(NULL); vec.add(4).writePointer(NULL); vec.add(8).writePointer(NULL); vec.add(12).writeU32(0);
    const [str, buf] = stdString(key);
    lockWorld(world);
    let ok;
    try { ok = getBlobVec(world.add(0xac), str, vec); } finally { unlockWorld(world); }
    if (!ok || buf === null) return null;
    const b = vec.readPointer(), e = vec.add(4).readPointer();
    const n = e.sub(b).toInt32();
    return n > 0 ? b.readByteArray(n) : new ArrayBuffer(0);
  },
  // World::saveZone on a generated zone, optionally marking it dirty (zone+0x75) first.
  saveZone(zx, zy, dirty) {
    const z = getZone(world, zx, zy);
    if (z.isNull()) return false;
    if (dirty) z.add(0x75).writeU8(1);
    lockWorld(world);
    try { saveZoneFn(world, z); } finally { unlockWorld(world); }
    return true;
  },
  saveEntities(rx, ry) { lockWorld(world); try { saveEntitiesFn(world, rx, ry); } finally { unlockWorld(world); } },
  // The server tick unloads (and saves) zones nobody requested between RPC calls, so a zone's
  // dump, its World::saveZone and the blobs it produced are taken under one world lock. The
  // results wait in the snapshot table: 'full' (the dump), then one entry per requested key.
  generateZoneSave(zx, zy, dirty, region, keys) {
    lockWorld(world);
    stageSnapshots = {};
    try {
      genZone(world, zx, zy);
      const z = getZone(world, zx, zy);
      if (z.isNull()) return [];
      stageSnapshots.full = serializeZone(z);
      if (dirty) z.add(0x75).writeU8(1);
      saveZoneFn(world, z);
      if (region) saveEntitiesFn(world, region[0], region[1]);
      for (const key of keys) {
        const vec = Memory.alloc(16);
        vec.writePointer(NULL); vec.add(4).writePointer(NULL); vec.add(8).writePointer(NULL); vec.add(12).writeU32(0);
        const [str, buf] = stdString(key);
        if (!getBlobVec(world.add(0xac), str, vec) || buf === null) continue;
        const b = vec.readPointer(), e = vec.add(4).readPointer();
        const n = e.sub(b).toInt32();
        stageSnapshots[key] = n > 0 ? b.readByteArray(n) : new ArrayBuffer(0);
      }
      return Object.keys(stageSnapshots);
    } finally { unlockWorld(world); }
  },
  worldTime() { return [world.add(0x800160).readS32(), world.add(0x80015c).readS32()]; },
  setWorldDay(day) { world.add(0x800160).writeS32(day); },
  seed() { return world.add(0x800164).readS32(); },
  generateZone(zx, zy) {
    lockWorld(world);
    try {
      genZone(world, zx, zy);
      const z = getZone(world, zx, zy);
      if (z.isNull()) return null;
      return serializeZone(z);
    } finally { unlockWorld(world); }
  },
  noiseBatch(pairs) { const out = new Array(pairs.length); for (let i = 0; i < pairs.length; i++) out[i] = noise(pairs[i][0], pairs[i][1]); return out; },
  randSeq(seed, n) { c_srand(seed); const out = new Array(n); for (let i = 0; i < n; i++) out[i] = c_rand(); return out; },
  readBytes(addr, n) { return ptr(addr).readByteArray(n); },
  readS32(addr) { return ptr(addr).readS32(); },
  readRva(addr, n) { return Array.from(new Uint8Array(rva(addr).readByteArray(n))); },
};
"""


class Oracle:
    def __init__(self):
        self.pid = frida.spawn([str(GAME / "Server.exe")], cwd=str(GAME))
        self.session = frida.attach(self.pid)
        self.world = None
        js = AGENT_JS % {
            "image_base": IMAGE_BASE,
            "world_ctor": ADDR["World::World"],
            "noise": ADDR["valueNoise2D"],
            "gen_zone": ADDR["World::generateZone"],
            "get_zone": ADDR["World::getZone"],
            "lock": ADDR["World::lock"],
            "get_point": ADDR["World::getOrCreateClimatePoint"],
            "create_region": ADDR["World::createRegion"],
            "get_region": ADDR["World::getRegion"],
            "unlock": ADDR["World::unlock"],
            "zone_ctor": ADDR["Zone::Zone"],
        }
        self.script = self.session.create_script(js)
        self.script.on("message", self._on_message)
        self.script.load()
        frida.resume(self.pid)
        deadline = time.time() + 20
        while self.world is None and time.time() < deadline:
            time.sleep(0.05)
        if self.world is None:
            raise RuntimeError("World constructor was not observed within 20 s")

    def _on_message(self, message, data):
        if message.get("type") == "send" and message["payload"].get("type") == "world":
            self.world = int(message["payload"]["ptr"], 16)
        elif message.get("type") == "error":
            print(message, file=sys.stderr)

    @property
    def rpc(self):
        return self.script.exports_sync

    def close(self):
        try:
            frida.kill(self.pid)
        except Exception:
            pass


def f64_hex(v: float) -> str:
    return struct.pack("<d", v).hex()


def f32_hex(v: float) -> str:
    return struct.pack("<f", v).hex()


def sample_inputs(n: int, seed: int):
    """Inputs shaped like the generator's calls: (block * scale + offset) with offsets up to ~1e6."""
    rng = random.Random(seed)
    out = []
    while len(out) < n:
        kind = rng.random()
        if kind < 0.5:
            x = rng.uniform(-70000, 1_100_000)
            y = rng.uniform(-70000, 1_100_000)
        elif kind < 0.8:
            x = rng.randint(-100000, 20_000_000) * rng.choice([0.0005, 0.001, 0.0025, 0.005, 0.01, 0.02, 0.03, 0.04, 0.05, 0.08, 0.1, 0.4]) + rng.randint(0, 100000)
            y = rng.randint(-100000, 20_000_000) * rng.choice([0.0005, 0.001, 0.0025, 0.005, 0.01, 0.02, 0.03, 0.04, 0.05, 0.08, 0.1, 0.4]) + rng.randint(0, 100000)
        elif kind < 0.9:
            x = float(rng.randint(-1000, 1000))
            y = float(rng.randint(-1000, 1000))
        else:
            x = rng.uniform(-5, 5)
            y = rng.uniform(-5, 5)
        out.append((x, y))
    return out


def cmd_noise(args):
    o = Oracle()
    try:
        pairs = sample_inputs(args.count, args.seed)
        results = []
        for i in range(0, len(pairs), 2000):
            results.extend(o.rpc.noise_batch(pairs[i : i + 2000]))
        path = Path(args.out)
        path.parent.mkdir(parents=True, exist_ok=True)
        with open(path, "w") as f:
            f.write("# valueNoise2D golden samples from Server.exe 0x004d5d30: x_f64hex y_f64hex result_f32hex (little endian)\n")
            for (x, y), r in zip(pairs, results):
                f.write(f"{f64_hex(x)} {f64_hex(y)} {f32_hex(r)}\n")
        print(f"wrote {len(pairs)} samples to {path}")
    finally:
        o.close()


# Function specs for `fn`: name -> (address, abi, return type, arg spec, sampler).
# The sampler yields argument lists (matching the arg spec, with 'world' entries as None).
def _blocks(rng):
    """A block coordinate near typical generated areas plus a few extremes."""
    return rng.choice([rng.randint(0, 1 << 24), rng.randint(4_900_000, 5_000_000), rng.randint(8_000_000, 8_400_000), rng.randint(-300, 300)])


FN_SPECS = {
    "warpPosition": (0x004D5A80, "thiscall", "pointer", ["world", "out_f64x2", "int", "int"],
                     lambda rng: [None, None, _blocks(rng), _blocks(rng)]),
    "colorA_00522320": (0x00522320, "mscdecl", "pointer", ["out_f32x3", "int", "int"],
                        lambda rng: [None, _blocks(rng), _blocks(rng)]),
    "colorB_0052d5d0": (0x0052D5D0, "thiscall", "pointer", ["world", "out_f32x3", "int", "int"],
                        lambda rng: [None, None, _blocks(rng), _blocks(rng)]),
    "colorC_004f82d0": (0x004F82D0, "thiscall", "pointer", ["world", "out_f32x3", "int", "int"],
                        lambda rng: [None, None, _blocks(rng), _blocks(rng)]),
    "colorD_0052d870": (0x0052D870, "mscdecl", "pointer", ["out_f32x3", "int", "int"],
                        lambda rng: [None, _blocks(rng), _blocks(rng)]),
    "climateA": (0x004F8570, "thiscall", "float", ["world", "int", "int"], lambda rng: [None, _blocks(rng), _blocks(rng)]),
    "climateB": (0x004F8B40, "thiscall", "float", ["world", "int", "int"], lambda rng: [None, _blocks(rng), _blocks(rng)]),
    "climateC": (0x00522E20, "thiscall", "float", ["world", "int", "int"], lambda rng: [None, _blocks(rng), _blocks(rng)]),
    "edgeDist_00522840": (0x00522840, "thiscall", "float", ["world", "int", "int"], lambda rng: [None, _blocks(rng), _blocks(rng)]),
    "heightA_0052cd50": (0x0052CD50, "thiscall", "float", ["world", "int", "int", "int"], lambda rng: [None, _blocks(rng), _blocks(rng), 0]),
    "heightB_0052d990": (0x0052D990, "thiscall", "float", ["world", "int", "int"], lambda rng: [None, _blocks(rng), _blocks(rng)]),
    "cellFalloff_004d19f0": (0x004D19F0, "thiscall", "float", ["world", "int", "int"], lambda rng: [None, _blocks(rng), _blocks(rng)]),
    "baseHeight": (0x004F9B70, "thiscall", "float", ["world", "int", "int", "int"], lambda rng: [None, _blocks(rng), _blocks(rng), 0]),
    "mountainFactor_00523d80": (0x00523D80, "thiscall", "float", ["world", "int", "int", "int"], lambda rng: [None, _blocks(rng), _blocks(rng), 0]),
    "terrainColor": (0x0052D030, "thiscall", "pointer", ["world", "out_f32x3", "int", "int", "int", "int"],
                     lambda rng: [None, None, _blocks(rng), _blocks(rng), rng.randint(-120, 320), 0]),
    "rockColor_004fae90": (0x004FAE90, "thiscall", "pointer", ["world", "out_f32x3", "int", "int", "int", "int"],
                           lambda rng: [None, None, _blocks(rng), _blocks(rng), rng.randint(-120, 320), 0]),
    "surfaceBlock": (0x004F9450, "thiscall", "void", ["world", "out_u32x1", "int", "int", "int", "float", "float", "int"],
                     lambda rng: [None, None, _blocks(rng), _blocks(rng), rng.randint(-120, 320), rng.random(), rng.random(), 0]),
    "spawnLevel_004d2340": (0x004D2340, "thiscall", "uint", ["world", "uint", "uint", "uint", "uint"],
                            lambda rng: [None] + (lambda bx, by: [(bx << 16) & 0xffffffff, (bx >> 16) & 0xffffffff, (by << 16) & 0xffffffff, (by >> 16) & 0xffffffff])(_blocks(rng), _blocks(rng))),
}


NEEDS_POINTS = {"climateA", "climateB", "climateC", "edgeDist_00522840", "heightA_0052cd50", "heightB_0052d990", "cellFalloff_004d19f0", "baseHeight",
                "mountainFactor_00523d80", "terrainColor", "rockColor_004fae90", "surfaceBlock", "spawnLevel_004d2340"}


def cmd_fn(args):
    addr, abi, ret, spec, sampler = FN_SPECS[args.name]
    rng = random.Random(args.seed)
    samples = [sampler(rng) for _ in range(args.count)]
    o = Oracle()
    try:
        if args.name in NEEDS_POINTS:
            regions = set()
            for sample in samples:
                ints = [v for a, v in zip(spec, sample) if a == "int"]
                rx, ry = ints[0] >> 14, ints[1] >> 14
                for dx in (-1, 0, 1):
                    for dy in (-1, 0, 1):
                        if 0 <= rx + dx < 1024 and 0 <= ry + dy < 1024:
                            regions.add((rx + dx, ry + dy))
            regions = sorted(regions)
            for i in range(0, len(regions), 500):
                o.rpc.climate_points(regions[i : i + 500])
            print(f"created {len(regions)} climate points")
        results = []
        for i in range(0, len(samples), 1000):
            results.extend(o.rpc.call_batch(addr, abi, ret, spec, samples[i : i + 1000]))
        path = Path(args.out)
        path.parent.mkdir(parents=True, exist_ok=True)
        with open(path, "w") as f:
            f.write(f"# {args.name} golden samples from Server.exe 0x{addr:08x}, world seed {o.rpc.seed()}. "
                    f"Columns: inputs ({' '.join(a for a in spec if not (a == 'world' or a.startswith('out_')))}) then outputs as little-endian hex\n")
            for sample, r in zip(samples, results):
                ins = [(f32_hex(v) if a == "float" else str(v)) for a, v in zip(spec, sample) if not (a == "world" or a.startswith("out_"))]
                outs = []
                if ret == "float":
                    outs.append(f32_hex(r["ret"]))
                elif ret in ("double",):
                    outs.append(f64_hex(r["ret"]))
                elif ret in ("int", "uint"):
                    outs.append(str(r["ret"]))
                for a, vals in zip([a for a in spec if a.startswith("out_")], r["outs"]):
                    outs.extend(("%08x" % v) if "u32" in a else (f32_hex if "f32" in a else f64_hex)(v) for v in vals)
                f.write(" ".join(ins + outs) + "\n")
        print(f"wrote {len(samples)} samples to {path}")
    finally:
        o.close()


def cmd_points(args):
    """Golden climate points: one line per region with the seven fields."""
    rng = random.Random(args.seed)
    regions = sorted({(rng.randint(0, 1023), rng.randint(0, 1023)) for _ in range(args.count)} | {(299, 299), (300, 300), (512, 512), (0, 0), (1023, 1023)})
    o = Oracle()
    try:
        rows = []
        for i in range(0, len(regions), 500):
            rows.extend(o.rpc.climate_points(regions[i : i + 500]))
        path = Path(args.out)
        path.parent.mkdir(parents=True, exist_ok=True)
        with open(path, "w") as f:
            f.write(f"# climate points from Server.exe 0x0050b870, world seed {o.rpc.seed()}: rx ry x y flag A_f32hex B_f32hex seed elevation\n")
            for (rx, ry), r in zip(regions, rows):
                f.write(f"{rx} {ry} {r[0]} {r[1]} {r[2]} {r[3]:08x} {r[4]:08x} {r[5]} {r[6]}\n")
        print(f"wrote {len(regions)} points to {path}")
    finally:
        o.close()


def cmd_cos(args):
    """Sample-level cos goldens: x_f64hex cos_f64hex, inputs as in `hash cos`."""
    o = Oracle()
    try:
        xs = [args.lo + (float(((i * 0x9E3779B1) & 0xFFFFFFFF)) / 4294967296.0) * (args.hi - args.lo) for i in range(args.count)]
        out = []
        for i in range(0, len(xs), 5000):
            out.extend((o.rpc.sin_batch if args.func == "sin" else o.rpc.cos_batch)(xs[i : i + 5000]))
        with open(args.out, "w") as f:
            f.write(f"# MSVCR110 {args.func}: x_f64hex {args.func}_f64hex (little endian)" + chr(10))
            for x, c in zip(xs, out):
                f.write(f"{f64_hex(x)} {f64_hex(c)}" + chr(10))
        print(f"wrote {len(xs)} samples to {args.out}")
    finally:
        o.close()


def cmd_region(args):
    """Golden regions. Each region is created in the listed order (order matters: baseHeight
    reads the cells of regions that already exist)."""
    o = Oracle()
    try:
        regions = [tuple(int(v) for v in r.split(",")) for r in args.regions]
        out = o.rpc.create_regions(regions)
        nl = chr(10)
        with open(args.out, "w") as f:
            f.write(f"# regions from Server.exe createRegion 0x0050e080, world seed {o.rpc.seed()}, created in file order." + nl)
            f.write("# region rx ry level f10 variant | cell i x64 y64 radius_f32hex height_f32hex type variant id level extra | zone index kind sub seed level byte0c" + nl)
            for (rx, ry), r in zip(regions, out):
                if r is None:
                    f.write(f"region {rx} {ry} missing" + nl)
                    continue
                f.write(f"region {rx} {ry} {r[0]} {r[1]} {r[2]}" + nl)
                for i, c in enumerate(r[3]):
                    x64 = (c[1] << 32) | c[0]
                    y64 = (c[3] << 32) | c[2]
                    f.write(f"cell {i} {x64} {y64} {c[4]:08x} {c[5]:08x} {c[6]} {c[7]} {c[8]} {c[9]} {c[10]}" + nl)
                for z in r[4]:
                    f.write(f"zone {z[0]} {z[1]} {z[2]} {z[3]} {z[4]} {z[5]}" + nl)
        print(f"wrote {len(regions)} regions to {args.out}")
    finally:
        o.close()


def cmd_hash(args):
    o = Oracle()
    try:
        if args.what == "cos":
            print(f"cos {args.count} {o.rpc.cos_hash(args.count)}")
        elif args.what == "sin":
            print(f"sin {args.count} {o.rpc.sin_hash(args.count)}")
        else:
            print(f"noise {args.count} {o.rpc.noise_hash(args.count)}")
    finally:
        o.close()


def cmd_statics(args):
    """Runtime bytes of the static default blocks (initialised at startup, zero in the file)."""
    o = Oracle()
    try:
        for name, addr, n in [("getBlock defaults 0x583d0c (water, air, below)", 0x00583D0C, 12), ("setBlock default 0x583db0", 0x00583DB0, 4),
                              ("valley statics 0x5842bc/c0/c4", 0x005842BC, 12), ("blockAt defaults 0x583d18/20", 0x00583D18, 12)]:
            b = bytes(o.rpc.read_rva(addr, n))
            print(name, " ".join(f"{b[i]:3d}" for i in range(n)), "|", " ".join(b[i:i+4].hex() for i in range(0, n, 4)))
    finally:
        o.close()


def cmd_seeds(args):
    o = Oracle()
    try:
        w = o.world
        raw = bytes(o.rpc.read_bytes(w + 0x800164, 0x138))
        vals = struct.unpack("<%di" % (len(raw) // 4), raw)
        print("seed", vals[0])
        print("subseeds", " ".join(str(v) for v in vals[1:]))
    finally:
        o.close()


def cmd_zone(args):
    cfg = GAME / "server.cfg"
    saved = cfg.read_bytes()
    if args.seed is not None:
        cfg.write_text(str(args.seed))
    o = Oracle()
    try:
        print(f"world seed {o.rpc.seed()}")
        stages = args.stage.split(",")
        for zx, zy in args.zones:
            t0 = time.time()
            fired = o.rpc.generate_zone_stages(zx, zy, stages)
            for stage in stages:
                data = o.rpc.take_snapshot(stage) if stage in fired else None
                if data is None:
                    print(f"zone {zx},{zy} {stage}: stage did not fire")
                    continue
                path = Path(args.out_dir) / f"zone_{o.rpc.seed()}_{zx}_{zy}_{stage}.bin"
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(bytes(data))
                print(f"zone {zx},{zy} {stage}: {len(data)} bytes -> {path}")
            print(f"zone {zx},{zy}: {time.time() - t0:.1f}s")
    finally:
        o.close()
        cfg.write_bytes(saved)


def cmd_entity(args):
    """Golden samples of the entity delta codec: entity_cases.py generates the inputs, the original encodes and decodes."""
    import entity_cases
    o = Oracle()
    try:
        lines = ["# case full delta_len mask sha256(delta) sha256(prev) sha256(cur) trunc sha256(target after read) pos"]
        for i in range(args.count):
            c = entity_cases.make_case(i)
            delta = bytes(o.rpc.entity_delta(list(c.prev), list(c.cur), c.full))
            trunc = entity_cases.truncation(c, len(delta))
            r = o.rpc.entity_read(list(delta[:trunc]), list(c.target))
            after = bytes(r["bytes"])
            mask = int.from_bytes(delta[:8], "little") if len(delta) >= 8 else 0
            lines.append(f"{i} {int(c.full)} {len(delta)} {mask:016x} {hashlib.sha256(delta).hexdigest()} "
                         f"{hashlib.sha256(c.prev).hexdigest()} {hashlib.sha256(c.cur).hexdigest()} {trunc} "
                         f"{hashlib.sha256(after).hexdigest()} {r['pos']}")
            if i % 50 == 0:
                print(f"case {i}: delta {len(delta)} bytes, mask {mask:016x}, trunc {trunc}")
        Path(args.out).parent.mkdir(parents=True, exist_ok=True)
        Path(args.out).write_text("\n".join(lines) + "\n")
        print(f"wrote {args.count} cases to {args.out}")
    finally:
        o.close()


def cmd_save(args):
    """Runs the save-database scenario of save_scenario.py through the original and writes the goldens."""
    import save_scenario
    save_scenario.run(Oracle, GAME, Path(args.out_dir), Path(args.dump_dir) if args.dump_dir else None)


def cmd_rand(args):
    o = Oracle()
    try:
        seq = o.rpc.rand_seq(args.seed, args.count)
        print(" ".join(str(v) for v in seq))
    finally:
        o.close()


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)
    n = sub.add_parser("noise", help="dump valueNoise2D golden samples")
    n.add_argument("--count", type=int, default=20000)
    n.add_argument("--seed", type=int, default=1)
    n.add_argument("--out", required=True)
    n.set_defaults(fn=cmd_noise)
    r = sub.add_parser("rand", help="print the MSVC rand() sequence for a seed")
    r.add_argument("--seed", type=int, required=True)
    r.add_argument("--count", type=int, default=32)
    r.set_defaults(fn=cmd_rand)
    z = sub.add_parser("zone", help="generate zones through the original and dump their terrain")
    z.add_argument("--out-dir", default="analysis/oracle")
    z.add_argument("--stage", default="full", help="comma-separated stage names (see STAGES in the agent script); 'full' is the finished zone")
    z.add_argument("--seed", type=int, default=None, help="world seed: written to game/server.cfg for the run and restored afterwards")
    z.add_argument("zones", nargs="+", type=lambda s: tuple(int(v) for v in s.split(",")), help="zx,zy pairs")
    z.set_defaults(fn=cmd_zone)
    f = sub.add_parser("fn", help="dump golden samples of one function (see FN_SPECS)")
    f.add_argument("--name", required=True, choices=sorted(FN_SPECS))
    f.add_argument("--count", type=int, default=3000)
    f.add_argument("--seed", type=int, default=1)
    f.add_argument("--out", required=True)
    f.set_defaults(fn=cmd_fn)
    pt = sub.add_parser("points", help="dump golden climate points for random regions")
    pt.add_argument("--count", type=int, default=2000)
    pt.add_argument("--seed", type=int, default=1)
    pt.add_argument("--out", required=True)
    pt.set_defaults(fn=cmd_points)
    rg = sub.add_parser("region", help="create regions through the original and dump their cells")
    rg.add_argument("--out", required=True)
    rg.add_argument("regions", nargs="+", help="rx,ry in creation order")
    rg.set_defaults(fn=cmd_region)
    cs = sub.add_parser("cos", help="dump MSVCR110 cos samples over [0, pi)")
    cs.add_argument("--count", type=int, default=200000)
    cs.add_argument("--out", required=True)
    cs.add_argument("--lo", type=float, default=0.0)
    cs.add_argument("--hi", type=float, default=3.14159274101257324)
    cs.add_argument("--func", choices=["cos", "sin"], default="cos")
    cs.set_defaults(fn=cmd_cos)
    hs = sub.add_parser("hash", help="FNV-1a hash of MSVCR110 cos or valueNoise2D over a deterministic input sequence")
    hs.add_argument("what", choices=["cos", "sin", "noise"])
    hs.add_argument("--count", type=int, default=1000000)
    hs.set_defaults(fn=cmd_hash)
    st = sub.add_parser("statics", help="print the static default blocks")
    st.set_defaults(fn=cmd_statics)
    sd = sub.add_parser("seeds", help="print the world's seed and sub-seed table")
    sd.set_defaults(fn=cmd_seeds)
    en = sub.add_parser("entity", help="golden samples of the entity delta codec (entity_cases.py)")
    en.add_argument("--count", type=int, default=400)
    en.add_argument("--out", default=str(ROOT / "crates/cw-net/tests/golden/entity_delta.txt"))
    en.set_defaults(fn=cmd_entity)
    sv = sub.add_parser("save", help="run the save-database scenario (save_scenario.py) and write its goldens")
    sv.add_argument("--out-dir", default=str(ROOT / "crates/cw-world/tests/golden/save"))
    sv.add_argument("--dump-dir", default=None, help="also keep the format-6 zone dumps here")
    sv.set_defaults(fn=cmd_save)
    args = p.parse_args()
    args.fn(args)


if __name__ == "__main__":
    main()
