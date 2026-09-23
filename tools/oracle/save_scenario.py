r"""The save-database scenario run through the original by `server_oracle.py save`.

`crates/cw-world/tests/save.rs` replays the same scenario through the port and compares with
the goldens written here (`crates/cw-world/tests/golden/save/`). The constants below are the
shared contract; change both sides together.

Scenario (seed 26879, world day set to 10 before anything is generated, `game/Save` created so
World::load opened `Save/world_server_26879.db`):

1. Store crafted blobs: `zone32768_32768` (three ground items of which one has expired, three
   modified blocks of which one has expired and one removes the block under the zone's first
   flag-2 prop, the eight spawn word pairs, one static byte), `mission4096_4096` and
   `monster4096_4096` (a saved boss placed in zone 32769,32768, which shares the cell).
2. Generate 32768,32768 (loads the zone blob), 32769,32768 (the boss) and 40000,20000 (five
   generated ground items) and hash their format-6 dumps.
3. World::saveZone on each (the first is dirty from the load, the other two are marked dirty)
   and World::saveEntities on region 512,512 right after zone C; read the blobs back. Each
   zone's steps run under one world lock, since the server tick unloads idle zones.
"""
import hashlib
import shutil
import struct
import time
from pathlib import Path

SEED = 26879
DAY = 10
ZONE_B = (32768, 32768)   # loads the crafted zone blob
ZONE_C = (32769, 32768)   # gets the saved boss
ZONE_A = (40000, 20000)   # has generated ground items
CELL_C = (4096, 4096)     # rx*8+cx, ry*8+cy of zones B and C
CELL_OTHER = (4096, 4097)
REGION = (512, 512)
# The support block of the first flag-2 prop of zone B (kind 6 at block 8388608, 8388799, -63).
PROP_SUPPORT = (8388608, 8388799, -64)


def zone_key(zx, zy):
    return f"zone{zx}_{zy}"


def mission_key(a, b):
    return f"mission{a}_{b}"


def monster_key(a, b):
    return f"monster{a}_{b}"


def item_bytes(item_type, sub_type, level, material=0, rarity=0, modifier=0):
    """A raw 0x118-byte item with every constructor field set and the rest zero."""
    b = bytearray(0x118)
    b[0] = item_type
    b[1] = sub_type
    struct.pack_into("<i", b, 4, modifier)
    b[0xc] = rarity
    b[0xd] = material
    struct.pack_into("<H", b, 0x10, level)
    return bytes(b)


def fixed(block, half=True):
    return (block << 16) + (32768 if half else 0)


# (item, x, y, z, rotation, f134, b138, f13c, f140, f144)
CRAFTED_ITEMS = [
    (item_bytes(1, 1, 5), fixed(32768 * 256 + 100), fixed(32768 * 256 + 100), fixed(120, False), 1.5, 0.1, 2, 0, 0, -1),
    (item_bytes(0xb, 0x13, 3, material=9), fixed(32768 * 256 + 101), fixed(32768 * 256 + 100), fixed(121, False), 0.0, 0.1, 0, 5, 6, 9),
    (item_bytes(0xc, 0, 1, material=10), fixed(32768 * 256 + 102), fixed(32768 * 256 + 100), fixed(122, False), 2.5, 0.1, 2, 0, 0, 5),
]
# (x, y, z, block, day)
CRAFTED_BLOCKS = [
    (PROP_SUPPORT[0], PROP_SUPPORT[1], PROP_SUPPORT[2], bytes([0, 0, 0, 0]), -1),
    (32768 * 256 + 92, 32768 * 256 + 92, 120, bytes([200, 100, 50, 0x01]), 8),
    (32768 * 256 + 42, 32768 * 256 + 42, 90, bytes([1, 2, 3, 0x06]), 2),
]
ZONE_B_SPAWNS = 8
ZONE_B_STATICS = 1
# (f30, f34, f38, f3c, b40, b41, f44, f48, f4c)
CRAFTED_MISSION = (1, 1, 2, 3, 4, 0, 5, 6, 7)
# (kind, level, b5c, zone)
CRAFTED_MONSTER = (0x68, 12, 1, ZONE_C)


def crafted_zone_b():
    out = bytearray(struct.pack("<I", 1))
    out += struct.pack("<I", len(CRAFTED_ITEMS))
    for it, x, y, z, rot, f134, b138, f13c, f140, f144 in CRAFTED_ITEMS:
        out += it + struct.pack("<qqqffBiii", x, y, z, rot, f134, b138, f13c, f140, f144)
    out += struct.pack("<I", len(CRAFTED_BLOCKS))
    for x, y, z, block, day in CRAFTED_BLOCKS:
        out += struct.pack("<iii", x, y, z) + block + struct.pack("<i", day)
    out += struct.pack("<I", ZONE_B_SPAWNS)
    for i in range(ZONE_B_SPAWNS):
        out += struct.pack("<II", 3 * i + 1, 7 * i + 2)
    out += struct.pack("<I", ZONE_B_STATICS)
    out += bytes([7] * ZONE_B_STATICS)
    return bytes(out)


def crafted_mission_c():
    f30, f34, f38, f3c, b40, b41, f44, f48, f4c = CRAFTED_MISSION
    return struct.pack("<IiiiiBBiiq", 1, f30, f34, f38, f3c, b40, b41, f44, f48, f4c)


def crafted_monster_c():
    kind, level, b5c, (zx, zy) = CRAFTED_MONSTER
    return struct.pack("<IiiBq", 1, kind, level, b5c, (zy << 32) | (zx & 0xffffffff))


def run(oracle_cls, game, out_dir, dump_dir):
    cfg = game / "server.cfg"
    saved_cfg = cfg.read_bytes()
    save_dir = game / "Save"
    if save_dir.exists():
        raise SystemExit(f"{save_dir} exists; remove it first (the scenario needs a fresh database)")
    save_dir.mkdir()
    cfg.write_text(str(SEED))
    out_dir.mkdir(parents=True, exist_ok=True)
    if dump_dir:
        dump_dir.mkdir(parents=True, exist_ok=True)
    o = None
    try:
        o = oracle_cls()
        rpc = o.rpc
        assert rpc.seed() == SEED, rpc.seed()
        assert rpc.db_open(), "World::load did not open the save database"
        rpc.set_world_day(DAY)
        assert rpc.world_time()[0] == DAY
        blobs = {
            zone_key(*ZONE_B): crafted_zone_b(),
            mission_key(*CELL_C): crafted_mission_c(),
            monster_key(*CELL_C): crafted_monster_c(),
        }
        for key, data in blobs.items():
            assert rpc.put_blob(key, list(data)), key
            back = bytes(rpc.get_blob(key))
            assert back == data, f"{key}: round trip through the database changed the bytes"
        (out_dir / "crafted_zone_B.blob").write_bytes(blobs[zone_key(*ZONE_B)])
        (out_dir / "crafted_mission_C.blob").write_bytes(blobs[mission_key(*CELL_C)])
        (out_dir / "crafted_monster_C.blob").write_bytes(blobs[monster_key(*CELL_C)])

        # The server tick unloads (saving) zones between RPC calls, so each zone's dump, its
        # World::saveZone and the resulting blobs are produced under one lock.
        hashes = []
        blobs_out = {}
        plan = [
            ("zone_B", ZONE_B, False, None, [zone_key(*ZONE_B)]),
            ("zone_C", ZONE_C, True, REGION, [zone_key(*ZONE_C), mission_key(*CELL_C), monster_key(*CELL_C), mission_key(*CELL_OTHER), monster_key(*CELL_OTHER)]),
            ("zone_A", ZONE_A, True, None, [zone_key(*ZONE_A)]),
        ]
        for name, (zx, zy), dirty, region, keys in plan:
            t0 = time.time()
            got = rpc.generate_zone_save(zx, zy, dirty, list(region) if region else None, keys)
            assert "full" in got, f"{name}: zone not generated"
            data = bytes(rpc.take_snapshot("full"))
            h = hashlib.sha256(data).hexdigest()
            hashes.append(f"{name} {zx} {zy} {h}")
            if dump_dir:
                (dump_dir / f"save_{name}_{zx}_{zy}_full.bin").write_bytes(data)
            print(f"{name} {zx},{zy}: {len(data)} bytes, {time.time() - t0:.1f}s, sha256 {h[:16]}")
            for key in keys:
                assert key in got, f"{key} was not written"
                blobs_out[key] = bytes(rpc.take_snapshot(key))
        header = "# name zx zy sha256 of the format-6 dump after generation in the scenario"
        (out_dir / "dumps.txt").write_text("\n".join([header] + hashes) + "\n")
        for name, key in [
            ("saved_zone_B", zone_key(*ZONE_B)),
            ("saved_zone_C", zone_key(*ZONE_C)),
            ("saved_zone_A", zone_key(*ZONE_A)),
            ("saved_mission_C", mission_key(*CELL_C)),
            ("saved_monster_C", monster_key(*CELL_C)),
            ("saved_mission_other", mission_key(*CELL_OTHER)),
            ("saved_monster_other", monster_key(*CELL_OTHER)),
        ]:
            (out_dir / f"{name}.blob").write_bytes(blobs_out[key])
            print(f"{name} ({key}): {len(blobs_out[key])} bytes")
    finally:
        if o is not None:
            o.close()
        cfg.write_bytes(saved_cfg)
        for _ in range(50):  # the killed process releases the database file shortly after
            shutil.rmtree(save_dir, ignore_errors=True)
            if not save_dir.exists():
                break
            time.sleep(0.1)
