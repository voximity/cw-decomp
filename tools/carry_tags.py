"""Copy Server.exe tags onto matched Cube.exe functions (post-pass after classify.py).

Usage:
    .venv/Scripts/python.exe tools/match_functions.py --dump matches.json
    .venv/Scripts/python.exe tools/carry_tags.py matches.json [--dst Cube.exe] [--min-conf 0.9] [--dry-run]

Reads the pair dump written by tools/match_functions.py --dump (source address ->
{dst, conf, ...}), analysis/tags/<src>.json and analysis/tags/<dst>.json. For every pair with
conf >= --min-conf where the destination tag is weak (`unknown`, or `crt` which classify.py
also uses for "shared helper" guesses) and the source tag is more specific, the destination
takes the source tag with confidence `derived` and reason `matched <src> 0x... (conf N)`.

Allowed directions:
  unknown -> cube, plasma, abstr, sqlite, freetype, ppl, crt
  crt     -> cube, plasma, abstr, sqlite, freetype, ppl
Never touched: `eh`, `thunk`, anything already tagged as game/engine/library code, and the
source binary's tags. Only analysis/tags/<dst>.json is rewritten.
"""
import argparse
import collections
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SPECIFIC = {"cube", "plasma", "abstr", "sqlite", "freetype", "ppl"}
CARRY = {"unknown": SPECIFIC | {"crt"}, "crt": SPECIFIC}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dump", help="JSON written by tools/match_functions.py --dump")
    ap.add_argument("--src", default="Server.exe")
    ap.add_argument("--dst", default="Cube.exe")
    ap.add_argument("--min-conf", type=float, default=0.9)
    ap.add_argument("--dry-run", action="store_true")
    a = ap.parse_args()

    pairs = json.loads(Path(a.dump).read_text())
    src_tags = json.loads((ROOT / "analysis/tags" / f"{a.src}.json").read_text())["tags"]
    dst_path = ROOT / "analysis/tags" / f"{a.dst}.json"
    dst_doc = json.loads(dst_path.read_text())
    dst_tags = dst_doc["tags"]

    changes = collections.Counter()
    skipped = collections.Counter()
    for s, p in pairs.items():
        d = "0x%08x" % int(p["dst"], 16)
        s = "0x%08x" % int(s, 16)
        if p["conf"] < a.min_conf or s not in src_tags or d not in dst_tags:
            continue
        ts, td = src_tags[s]["tag"], dst_tags[d]["tag"]
        if ts == td:
            continue
        if ts in CARRY.get(td, ()):
            dst_tags[d] = {"tag": ts, "confidence": "derived", "reason": f"matched {a.src} {s} (conf {p['conf']})"}
            changes[(td, ts)] += 1
        else:
            skipped[(td, ts)] += 1

    print(f"{a.dst}: {sum(changes.values())} tags carried from {a.src} (conf >= {a.min_conf})")
    for (f, t), n in sorted(changes.items()):
        print(f"  {f:8s} -> {t:8s} {n:5d}")
    if skipped:
        print("  disagreements left alone (dst -> src):")
        for (f, t), n in sorted(skipped.items()):
            print(f"    {f:8s} vs {t:8s} {n:5d}")
    if not a.dry_run:
        dst_doc["carried_from"] = {"src": a.src, "min_conf": a.min_conf, "count": sum(changes.values())}
        dst_path.write_text(json.dumps(dst_doc, indent=0))


if __name__ == "__main__":
    main()
