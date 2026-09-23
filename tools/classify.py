"""Tag every function in a Ghidra export as game, engine, library, or runtime code.

Usage:
    .venv/bin/python tools/classify.py analysis/export/Server.exe.json [--sqlite-literals PATH]

Writes analysis/tags/<binary>.json mapping entry address -> {tag, confidence, reason}
and prints a coverage table. Tags:

  eh        exception-handling fragments Ghidra split out (Unwind@..., Catch_All@...)
  thunk     jump thunks to imports or other functions
  crt       statically linked MSVC runtime pieces and std:: template instantiations
  sqlite    SQLite 3.7.15 amalgamation
  freetype  FreeType (client only)
  ppl       Concurrency runtime / Parallel Patterns Library instantiations
  cube      game namespace
  plasma    engine namespace
  abstr     abstr helper namespace
  unknown   nothing decided yet

Seeds come from names, vtable membership, and string references. Then tags propagate
along the call graph: an untagged function whose callers all share one tag inherits it,
and a function that only calls into one library tag joins that library. Repeats to a fixpoint.
"""
import argparse
import collections
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LIB_TAGS = {"sqlite", "freetype", "crt", "ppl"}
CODE_TAGS = {"cube", "plasma", "abstr"}
CRT_NAME_PREFIXES = ("__", "_atexit", "_onexit", "Facet_Register", "operator_new", "operator_delete",
                     "_Reset_back", "_Set_back", "_Uninitialized", "FID_conflict", "`", "Millisecs")
FREETYPE_MARKERS = ("FT_CURVE_TAG", "truetype", "TrueType", "cff", "sfnt", "glyf", "hmtx", "psnames", "autofit")
GENERIC = {"%s", "%d", "%s%s", "%.*s", "%lld", "%u", "%x", "%c", "true", "false", "null", "NULL", "main",
           "name", "type", "value", "error", "unknown", "temp", "test", "table", "index", "view", "trigger"}


CRT_STRINGS = {"bad allocation", "string too long", "vector<T> too long", "invalid string position",
               "map/set<T> too long", "bad cast", "invalid stoi argument", "deque<T> too long", "list<T> too long",
               "bad function call", "invalid hash bucket count", "unordered_map/set<T> too long"}
ASSET_SUFFIXES = (".cub", ".wav", ".png", ".plx", ".db", ".xml", ".dat", ".bmp", ".ttf")


def is_game_string(s, sqlite_lits):
    if s in sqlite_lits or s in CRT_STRINGS or len(s) < 4:
        return False
    if s.endswith(ASSET_SUFFIXES):
        return True
    if s.startswith(("%", "\\", "<", ".?A", "?", "__")):
        return False
    letters = sum(c.isalpha() for c in s)
    return letters >= 4 and " " in s or letters >= 8


def load_sqlite_literals(path):
    if path and Path(path).exists():
        lits = set(json.loads(Path(path).read_text()))
        return {s for s in lits if len(s) >= 6 and s not in GENERIC}
    return set()


def classify(export_path: Path, sqlite_lits: set):
    d = json.loads(export_path.read_text())
    F = {f["entry"]: f for f in d["functions"]}
    S = d["strings"]
    tags = {}

    def tag(entry, t, conf, reason):
        if entry in F and entry not in tags:
            tags[entry] = {"tag": t, "confidence": conf, "reason": reason}

    # --- seeds -----------------------------------------------------------------------------
    for v in d["vtables"]:
        top = v["class"].split("::")[0]
        t = {"cube": "cube", "plasma": "plasma", "abstr": "abstr", "std": "crt", "type_info": "crt",
             "Concurrency": "ppl"}.get(top)
        if "<lambda_" in v["class"]:
            # std::function / PPL wrappers around lambdas: the bodies are whoever wrote the lambda.
            t = "plasma" if "plasma::" in v["class"] else "cube"
            conf = "derived"
        else:
            conf = "high"
        if t:
            for e in v["entries"]:
                tag(e, t, conf, f"vtable {v['class'][:60]}")
    for e, f in F.items():
        n = f["name"]
        if n.startswith(("Unwind", "Catch_All", "Catch")):
            tag(e, "eh", "high", "EH fragment")
        elif f["thunk"]:
            tag(e, "thunk", "high", "thunk")
        elif f["namespace"] not in ("Global",) and not f["namespace"].endswith(".DLL"):
            top = f["namespace"].split("::")[0]
            if top in ("std", "Concurrency", "CRefTime"):
                tag(e, "crt" if top != "Concurrency" else "ppl", "high", f"namespace {f['namespace']}")
        elif n.startswith(CRT_NAME_PREFIXES):
            tag(e, "crt", "high", f"CRT name {n}")
    for e, f in F.items():
        if e in tags:
            continue
        strs = [S[k] for k in f["strings"] if k in S]
        sq = [s for s in strs if s in sqlite_lits]
        ft = [s for s in strs if any(m in s for m in FREETYPE_MARKERS)]
        game = [s for s in strs if is_game_string(s, sqlite_lits)]
        if sq and len(sq) >= max(1, len(strs) // 2):
            tag(e, "sqlite", "high", f"sqlite string {sq[0][:40]!r}")
        elif ft:
            tag(e, "freetype", "high", f"freetype string {ft[0][:40]!r}")
        elif game and not sq:
            tag(e, "cube", "derived", f"game string {game[0][:40]!r}")

    # --- propagation ---------------------------------------------------------------------
    callers = collections.defaultdict(set)
    for e, f in F.items():
        for c in f["callees"]:
            if c in F:
                callers[c].add(e)
    changed = True
    rounds = 0
    while changed and rounds < 50:
        changed = False
        rounds += 1
        for e, f in F.items():
            if e in tags:
                continue
            cs = {tags[c]["tag"] for c in callers[e] if c in tags}
            untagged_callers = [c for c in callers[e] if c not in tags]
            real_callers = [c for c in callers[e] if c not in tags or tags[c]["tag"] not in ("eh", "thunk")]
            if real_callers and not untagged_callers and len(cs - {"eh", "thunk"}) == 1:
                t = next(iter(cs - {"eh", "thunk"}))
                tag(e, t, "derived", f"all {len(real_callers)} callers are {t}")
                changed = True
                continue
            # library closure: only calls library functions of one tag and nothing tagged as game
            callee_tags = {tags[c]["tag"] for c in f["callees"] if c in tags}
            lib = callee_tags & {"sqlite", "freetype"}
            if f["callees"] and lib and len(lib) == 1 and not (callee_tags & CODE_TAGS) \
               and all(c in tags for c in f["callees"] if c in F) and not untagged_callers and cs <= lib | {"eh", "thunk"}:
                t = next(iter(lib))
                tag(e, t, "derived", f"only calls {t}")
                changed = True
    # --- reachability closure for self-contained libraries ------------------------------
    # Everything reachable from a library's proven functions, without passing through game
    # code, and not reachable from game code without passing through the library, is library.
    def reach(seeds, stop):
        seen = set(seeds)
        stack = list(seeds)
        while stack:
            e = stack.pop()
            for c in F[e]["callees"]:
                if c in F and c not in seen and tags.get(c, {}).get("tag") not in stop:
                    seen.add(c)
                    stack.append(c)
        return seen
    game_seeds = [e for e, t in tags.items() if t["tag"] in CODE_TAGS]
    for lib in ("sqlite", "freetype"):
        lib_seeds = [e for e, t in tags.items() if t["tag"] == lib and t["confidence"] == "high"]
        if not lib_seeds:
            continue
        from_lib = reach(lib_seeds, CODE_TAGS | {"crt"})
        from_game = reach(game_seeds, {lib})
        for e in from_lib - from_game:
            tag(e, lib, "derived", f"reachable only from {lib}")
        for e in from_lib & from_game:
            if e not in tags:
                tag(e, "crt", "derived", f"shared helper reached from both {lib} and game code")
    # --- function pointer tables in data: neighbours share a tag ---------------------------
    try:
        import pefile
        pe = pefile.PE(str(ROOT / "game" / d["binary"]), fast_load=True)
        base = pe.OPTIONAL_HEADER.ImageBase
        entries = {int(e, 16): e for e in F}
        for sec in pe.sections:
            if sec.Name.rstrip(b"\0") not in (b".rdata", b".data"):
                continue
            data = sec.get_data()
            va0 = base + sec.VirtualAddress
            ptrs = []
            for i in range(0, len(data) - 3, 4):
                v = int.from_bytes(data[i:i+4], "little")
                if v in entries:
                    ptrs.append((va0 + i, entries[v]))
            for idx, (addr, e) in enumerate(ptrs):
                if e in tags:
                    continue
                near = [tags[x]["tag"] for a, x in ptrs[max(0, idx-8):idx+9] if x in tags and abs(a - addr) <= 64]
                near = [t for t in near if t not in ("eh", "thunk", "unknown")]
                if len(near) >= 2 and len(set(near)) == 1:
                    tag(e, near[0], "derived", f"function pointer table at 0x{addr:08x} is {near[0]}")
    except Exception as ex:  # pefile missing or binary absent: skip this rule
        print("  (pointer-table rule skipped:", ex, ")")
    # --- PPL instantiations: talk to Concurrency::details and nothing game-tagged --------------
    for e, f in F.items():
        if e in tags:
            continue
        if any("Concurrency::" in x for x in f["calls_external"]) and \
           not any(tags.get(c, {}).get("tag") in CODE_TAGS for c in f["callees"]):
            tag(e, "ppl", "derived", "calls Concurrency runtime")
    # second propagation pass now that more seeds exist
    changed = True
    while changed:
        changed = False
        for e, f in F.items():
            if e in tags:
                continue
            cs = {tags[c]["tag"] for c in callers[e] if c in tags} - {"eh", "thunk"}
            if callers[e] and all(c in tags for c in callers[e]) and len(cs) == 1:
                tag(e, next(iter(cs)), "derived", f"all {len(callers[e])} callers are {next(iter(cs))}")
                changed = True
    for e in F:
        tag(e, "unknown", "none", "")

    out_dir = ROOT / "analysis" / "tags"
    out_dir.mkdir(parents=True, exist_ok=True)
    out = out_dir / export_path.name
    out.write_text(json.dumps({"binary": d["binary"], "rounds": rounds, "tags": tags}, indent=0))

    counts = collections.Counter(t["tag"] for t in tags.values())
    conf = collections.Counter((t["tag"], t["confidence"]) for t in tags.values())
    size = collections.Counter()
    for e, t in tags.items():
        size[t["tag"]] += F[e]["size"]
    print(f"{d['binary']}: {len(F)} functions, {rounds} propagation rounds")
    print(f"  {'tag':9s} {'functions':>9s} {'high':>6s} {'derived':>8s} {'bytes':>9s}")
    for t, n in counts.most_common():
        print(f"  {t:9s} {n:9d} {conf[(t,'high')]:6d} {conf[(t,'derived')]:8d} {size[t]:9d}")
    return out


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("exports", nargs="+")
    ap.add_argument("--sqlite-literals", default=None)
    a = ap.parse_args()
    lits = load_sqlite_literals(a.sqlite_literals)
    for x in a.exports:
        classify(Path(x), lits)
