r"""A minimal Cube World Alpha client for the protocol oracle.

Connects to a server (the original Server.exe or cw-server), performs the handshake and records
what the server sends for a while, optionally asking for a zone and a region to be "discovered".
Outputs go to a directory: `join.bin` (the raw entity block of the Join packet), `summary.json`,
one `update_<n>.bin` per non-empty ServerUpdate body (decompressed), and one `entity_<id>.bin`
per entity id with the first (full) entity delta the server sent for it.

    .venv\Scripts\python.exe tools/oracle/client_probe.py --port 12345 --out DIR --zone 32800,32800 --region 512,512

`--exe game/Server.exe` starts that server first (cwd = its directory) and kills it afterwards;
`--cmd "cargo run --release -p cw-server -- --port 12346 --no-save"` does the same for a command.
"""
import argparse
import json
import socket
import struct
import subprocess
import time
import zlib
from pathlib import Path

ENTITY_SIZE = 0x1168


def recv_exact(sock, n):
    out = bytearray()
    while len(out) < n:
        chunk = sock.recv(n - len(out))
        if not chunk:
            raise ConnectionError(f"connection closed after {len(out)} of {n} bytes")
        out += chunk
    return bytes(out)


def u32(sock):
    return struct.unpack("<I", recv_exact(sock, 4))[0]


def read_packet(sock):
    """One server->client packet as (id, payload dict)."""
    pid = u32(sock)
    if pid == 0:
        n = u32(sock)
        body = zlib.decompress(recv_exact(sock, n))
        return pid, {"entity_id": struct.unpack("<q", body[:8])[0], "delta": body[8:]}
    if pid == 2:
        return pid, {}
    if pid == 5:
        day, tod = struct.unpack("<II", recv_exact(sock, 8))
        return pid, {"day": day, "time": tod}
    if pid == 4:
        n = u32(sock)
        return pid, {"body": zlib.decompress(recv_exact(sock, n))}
    if pid == 10:
        sender = struct.unpack("<q", recv_exact(sock, 8))[0]
        n = u32(sock)
        return pid, {"sender": sender, "text": recv_exact(sock, 2 * n).decode("utf-16-le")}
    if pid == 0x12:
        return pid, {}
    raise ValueError(f"unexpected packet id {pid}")


def interact_packet(spec):
    """The 0x12c-byte Interact payload: a 0x118-byte item, zone x, zone y, index, a zero word, the type byte."""
    parts = spec.split(":")
    kind = int(parts[0])
    zx, zy, idx = (int(v) for v in parts[1].split(","))
    item = bytearray(0x118)
    if len(parts) > 2:
        t, s, lvl = (int(v) for v in parts[2].split(","))
        item[0] = t
        item[1] = s
        struct.pack_into("<H", item, 0x10, lvl)
    return bytes(item) + struct.pack("<iiiIB3x", zx, zy, idx, 0, kind)


def hit_packet(attacker, spec):
    """The 0x48-byte Hit payload: attacker, target, damage, critical, stun, something8, position, direction, skill_hit, hit_type, show_light."""
    parts = spec.split(":")
    target = int(parts[0])
    damage = float(parts[1])
    hit_type = int(parts[2]) if len(parts) > 2 else 0
    stun = int(parts[3]) if len(parts) > 3 else 0
    return struct.pack("<qqfB3xiiqqqfffBBBx", attacker, target, damage, 0, stun, 0, 0, 0, 0, 0.0, 0.0, 0.0, 0, hit_type, 0)


def run(args):
    proc = None
    if args.exe:
        exe = Path(args.exe)
        proc = subprocess.Popen([str(exe)], cwd=str(exe.parent), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    elif args.cmd:
        proc = subprocess.Popen(args.cmd, shell=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    summary = {"port": args.port, "version_sent": args.version, "packets": []}
    try:
        sock = None
        deadline = time.time() + 30
        while sock is None:
            try:
                sock = socket.create_connection(("127.0.0.1", args.port), timeout=5)
            except OSError:
                if time.time() > deadline:
                    raise
                time.sleep(0.5)
        sock.settimeout(20)
        sock.sendall(struct.pack("<II", 0x11, args.version))
        first = u32(sock)
        summary["first_id"] = first
        if first != 0x10:
            summary["note"] = "no join"
            (out / "summary.json").write_text(json.dumps(summary, indent=1))
            print(f"first packet id {first}; done")
            return
        zero = u32(sock)
        entity_id = struct.unpack("<q", recv_exact(sock, 8))[0]
        entity = recv_exact(sock, ENTITY_SIZE)
        (out / "join.bin").write_bytes(entity)
        seed_id = u32(sock)
        seed = u32(sock)
        summary.update({"join_zero": zero, "entity_id": entity_id, "seed_packet_id": seed_id, "seed": seed})
        print(f"joined as entity {entity_id}, seed {seed}")
        if args.pos:
            # An EntityUpdate (id 0) with only the position (mask bit 0), so the player stands
            # where the caller says instead of at the join position.
            x, y, z = (int(float(v) * 65536.0) for v in args.pos.split(","))
            body = zlib.compress(struct.pack("<q", entity_id) + struct.pack("<Q", 1) + struct.pack("<qqq", x, y, z))
            sock.sendall(struct.pack("<II", 0, len(body)) + body)
            print(f"position set to {args.pos}")
        if args.pet:
            # An EntityUpdate with only the equipment (mask bit 44, entity+0x2f0, 13 items of
            # 0x118 bytes): a pet item (type 0x13, sub type = the pet's creature type, level at
            # +0x10, XP at +4) in slot 12, named through the spirits' material bytes
            # (item+0x17 + i * 8, count at +0x114).
            parts = args.pet.split(",")
            kind = 0x13
            if parts[0] == "food":
                # A pet food item (type 0x14, sub type = the creature type it tames).
                kind = 0x14
                parts = parts[1:]
            ptype = int(parts[0])
            plevel = int(parts[1]) if len(parts) > 1 else 1
            pname = parts[2] if len(parts) > 2 else ""
            equip = bytearray(entity[0x2f0:0x2f0 + 0xe38])
            item = bytearray(0x118)
            item[0] = kind
            item[1] = ptype
            struct.pack_into("<h", item, 0x10, plevel)
            for i, ch in enumerate(pname.encode()[:15]):
                item[0x17 + i * 8] = ch
            struct.pack_into("<i", item, 0x114, len(pname.encode()[:15]))
            equip[12 * 0x118:13 * 0x118] = item
            body = zlib.compress(struct.pack("<q", entity_id) + struct.pack("<Q", 1 << 44) + bytes(equip))
            sock.sendall(struct.pack("<II", 0, len(body)) + body)
            print(f"pet item sent: type {ptype} level {plevel} name {pname!r}")
        t0 = time.time()
        sent_requests = False
        n_updates = 0
        counts = {}
        entity_seen = {}
        last_pos = {}
        entity_last_t = {}
        moves = sorted((float(m.split("@")[1]), m.split("@")[0]) for m in (args.move or []))
        while time.time() - t0 < args.wait:
            while moves and time.time() - t0 >= moves[0][0]:
                # A later position update (`--move X,Y,Z@SECONDS`).
                _, spec = moves.pop(0)
                x, y, z = (int(float(v) * 65536.0) for v in spec.split(","))
                body = zlib.compress(struct.pack("<q", entity_id) + struct.pack("<Q", 1) + struct.pack("<qqq", x, y, z))
                sock.sendall(struct.pack("<II", 0, len(body)) + body)
                print(f"moved to {spec} at {time.time() - t0:.1f}s")
            if not sent_requests and time.time() - t0 >= args.after:
                sent_requests = True
                if args.goto:
                    # Move the player next to a creature (its last reported position plus an
                    # offset) before the other requests.
                    gid, off = args.goto.split(":")
                    gid = int(gid)
                    dx, dy, dz = (float(v) for v in off.split(","))
                    if gid in last_pos:
                        gx, gy, gz = last_pos[gid]
                        x, y, z = gx + int(dx * 65536.0), gy + int(dy * 65536.0), gz + int(dz * 65536.0)
                        body = zlib.compress(struct.pack("<q", entity_id) + struct.pack("<Q", 1) + struct.pack("<qqq", x, y, z))
                        sock.sendall(struct.pack("<II", 0, len(body)) + body)
                        print(f"moved next to {gid}: {x / 65536.0:.2f},{y / 65536.0:.2f},{z / 65536.0:.2f}")
                    else:
                        print(f"goto: entity {gid} not seen")
                if args.zone:
                    zx, zy = (int(v) for v in args.zone.split(","))
                    sock.sendall(struct.pack("<Iii", 11, zx, zy))
                if args.region:
                    rx, ry = (int(v) for v in args.region.split(","))
                    sock.sendall(struct.pack("<Iii", 12, rx, ry))
                if args.mode is not None:
                    # The player's mode byte (mask bit 9, entity+0x58), e.g. 0x6a to ride.
                    body = zlib.compress(struct.pack("<q", entity_id) + struct.pack("<Q", 1 << 9) + bytes([args.mode]))
                    sock.sendall(struct.pack("<II", 0, len(body)) + body)
                    print(f"mode {args.mode:#x} sent")
                for spec in args.interact or []:
                    sock.sendall(struct.pack("<I", 6) + interact_packet(spec))
                for spec in args.hit or []:
                    count = int(spec.split("x")[1]) if "x" in spec else 1
                    for _ in range(count):
                        sock.sendall(struct.pack("<I", 7) + hit_packet(entity_id, spec.split("x")[0]))
                print(f"requests sent at {time.time() - t0:.1f}s")
            pid, p = read_packet(sock)
            counts[pid] = counts.get(pid, 0) + 1
            if pid == 5 and "first_time" not in summary:
                summary["first_time"] = p
            if pid == 4 and p["body"] != b"\0" * 52:
                n_updates += 1
                (out / f"update_{n_updates}.bin").write_bytes(p["body"])
                summary["packets"].append({"t": round(time.time() - t0, 2), "id": 4, "len": len(p["body"])})
                print(f"ServerUpdate {n_updates}: {len(p['body'])} bytes at {time.time() - t0:.1f}s")
            elif pid == 0:
                eid = p["entity_id"]
                if len(p["delta"]) >= 32 and struct.unpack_from("<Q", p["delta"], 0)[0] & 1:
                    last_pos[eid] = struct.unpack_from("<qqq", p["delta"], 8)
                if eid not in entity_seen:
                    entity_seen[eid] = 0
                    (out / f"entity_{eid}.bin").write_bytes(p["delta"])
                    summary["packets"].append({"t": round(time.time() - t0, 2), "id": 0, "entity": eid, "delta_len": len(p["delta"])})
                    print(f"entity {eid}: first update ({len(p['delta'])} bytes) at {time.time() - t0:.1f}s")
                entity_seen[eid] += 1
                entity_last_t[eid] = round(time.time() - t0, 2)
                if sent_requests and eid != entity_id:
                    # Every delta after the requests, length-prefixed, so the final state of
                    # the creatures the requests touched can be rebuilt.
                    with (out / f"after_{eid}.bin").open("ab") as f:
                        f.write(struct.pack("<I", len(p["delta"])) + p["delta"])
            elif pid == 10:
                summary["packets"].append({"t": round(time.time() - t0, 2), "id": 10, "sender": p["sender"], "text": p["text"]})
        summary["counts"] = {str(k): v for k, v in counts.items()}
        summary["entity_updates"] = {str(k): v for k, v in entity_seen.items()}
        summary["entity_last_t"] = {str(k): v for k, v in entity_last_t.items()}
        sock.close()
    finally:
        (out / "summary.json").write_text(json.dumps(summary, indent=1))
        if proc is not None:
            proc.kill()
            proc.wait()
    print(f"wrote {out}")


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--port", type=int, default=12345)
    p.add_argument("--out", required=True)
    p.add_argument("--version", type=int, default=3)
    p.add_argument("--zone", default=None)
    p.add_argument("--region", default=None)
    p.add_argument("--pos", default=None, help="X,Y,Z in blocks: report this position right after the join")
    p.add_argument("--interact", action="append", help="TYPE:ZX,ZY,IDX[:ITEMTYPE,SUB,LEVEL], an Interact packet (id 6) sent with the requests; repeatable")
    p.add_argument("--hit", action="append", help="TARGET:DAMAGE[:TYPE[:STUN]][xCOUNT], Hit packets (id 7) from the player sent with the requests; repeatable")
    p.add_argument("--goto", default=None, help="ID:DX,DY,DZ: at the request time, move the player to that entity's last reported position plus the offset (blocks)")
    p.add_argument("--move", action="append", help="X,Y,Z@SECONDS: a later position update of the player; repeatable")
    p.add_argument("--pet", default=None, help="TYPE[,LEVEL[,NAME]]: put a pet item in equipment slot 12 right after the join")
    p.add_argument("--mode", type=lambda v: int(v, 0), default=None, help="send this mode byte (entity+0x58) with the requests, e.g. 0x6a")
    p.add_argument("--wait", type=float, default=15.0, help="seconds to keep reading")
    p.add_argument("--after", type=float, default=8.0, help="seconds before the discovery requests")
    p.add_argument("--exe", default=None, help="start this server executable first")
    p.add_argument("--cmd", default=None, help="start this shell command first")
    run(p.parse_args())


if __name__ == "__main__":
    main()
