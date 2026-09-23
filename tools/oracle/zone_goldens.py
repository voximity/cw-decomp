r"""Writes crates/cw-world/tests/golden/zones_stages.txt from oracle zone dumps.

    .venv\Scripts\python.exe tools/oracle/zone_goldens.py <dump dir> [--stages a,b,c] [--zones [seed:]zx,zy ...]

Every `zone_<seed>_<zx>_<zy>_<stage>.bin` in the directory is hashed (SHA-256 of the whole
file); rows are ordered by the zone order given with --zones (the order the zones were generated
in, per seed) and by stage order.
"""
import argparse
import hashlib
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
OUT = ROOT / "crates/cw-world/tests/golden/zones_stages.txt"
STAGE_ORDER = ["terrain", "features", "layers", "dungeon_in", "dungeons", "decor1", "trees", "creatures", "decor2", "appearance", "full"]


def main():
    p = argparse.ArgumentParser()
    p.add_argument("dump_dir")
    p.add_argument("--stages", default=",".join(STAGE_ORDER))
    p.add_argument("--zones", nargs="*", default=None, help="[seed:]zx,zy in generation order per seed (default: file order)")
    p.add_argument("--out", default=str(OUT))
    a = p.parse_args()
    stages = a.stages.split(",")
    files = {}
    for f in Path(a.dump_dir).glob("zone_*.bin"):
        m = re.match(r"zone_(\d+)_(\d+)_(\d+)_(\w+)\.bin", f.name)
        if not m:
            continue
        seed, zx, zy, stage = int(m[1]), int(m[2]), int(m[3]), m[4]
        files[(seed, zx, zy, stage)] = hashlib.sha256(f.read_bytes()).hexdigest()
    # Zones given without a seed prefix belong to the seed with the most dumps.
    counts = {}
    for k in files:
        counts[k[0]] = counts.get(k[0], 0) + 1
    default_seed = max(counts, key=counts.get) if counts else 0

    def parse(z):
        seed = default_seed
        if ":" in z:
            seed, z = z.split(":")
            seed = int(seed)
        zx, zy = (int(v) for v in z.split(","))
        return seed, zx, zy

    zones = [parse(z) for z in a.zones] if a.zones else sorted({k[:3] for k in files})
    lines = ["# seed zx zy stage sha256 of the oracle dump (format 5); zones of one seed generated in this order in one world"]
    for seed, zx, zy in zones:
        for st in stages:
            key = (seed, zx, zy, st)
            if key in files:
                lines.append(f"{seed} {zx} {zy} {st} {files[key]}")
    Path(a.out).write_text("\n".join(lines) + "\n")
    print(f"wrote {len(lines) - 1} rows to {a.out}")


if __name__ == "__main__":
    main()
