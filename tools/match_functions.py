"""Match functions between two builds of the same source (Server.exe -> Cube.exe) and
carry the names recorded for the first into the second.

Usage (from the repo root):
    .venv/Scripts/python.exe tools/match_functions.py \
        [--src Server.exe] [--dst Cube.exe] [--threshold 0.9] [--dump PATH]

Reads analysis/export/<exe>.json, analysis/tags/<exe>.json, analysis/rtti/<exe>.json,
analysis/names/<src>.json and the PE files in game/. Writes
analysis/names/<dst>.json (confident carried names) and
analysis/names/<dst>.candidates.json (doubtful ones, for review), and prints statistics.
--dump writes every pair the matcher found (named or not) with its confidence and evidence.

Signals, in the order they are tried (strongest first):
  strings   identical multiset of referenced strings (plus named data refs such as vftables),
            unique on both sides among the functions still unmatched
  imports   identical multiset of external calls, unique on both sides, size within 5%
  vtable    the same RTTI class's vtable slot N in both binaries
  callgraph a matched pair's ordered callee (and function-pointer) lists line up and position k
            is unmatched on both sides; or every callee of a function is matched and exactly
            one unmatched function on the other side calls exactly the mapped set
  bytes     identical instruction stream after masking absolute addresses and call targets,
            unique on both sides; last resort a fuzzy instruction similarity >= 0.95 within a
            size bucket with a clear winner

Each accepted pair is then scored from every signal that holds for it (not only the one
that proposed it), combined as 1 - prod(1 - p):
  strings 0.8 (0.6 for a single string), imports 0.5, vtable 0.8, callgraph 0.5 (0.7 when two
  or more independent matched neighbours agree), byte similarity: exact 0.9, >=0.95 0.7,
  >=0.8 0.4. A pair with byte similarity below 0.5 has its score halved (the source differs:
  #ifdef'd server/client code or a different inline decision) unless strings agree.
Pairs at or above --threshold are confident.
"""
import argparse
import collections
import difflib
import hashlib
import json
import re
from pathlib import Path

import capstone
import pefile

ROOT = Path(__file__).resolve().parent.parent
HEX = re.compile(r"0x[0-9a-fA-F]+")
BRANCH = re.compile(r"^(j\w+|call|loop\w*)$")


class Binary:
    def __init__(self, exe):
        self.exe = exe
        ex = json.loads((ROOT / "analysis/export" / f"{exe}.json").read_text())
        self.image_base = int(ex["image_base"], 16)
        self.strings = dict(ex["strings"])
        self.funcs = {}
        for f in ex["functions"]:
            if f["external"]:
                continue
            self.funcs[f["entry"]] = f
        tags = json.loads((ROOT / "analysis/tags" / f"{exe}.json").read_text())["tags"]
        self.tag = {a: tags.get(a, {}).get("tag", "unknown") for a in self.funcs}
        self.rtti = json.loads((ROOT / "analysis/rtti" / f"{exe}.json").read_text())["classes"]
        self.pe = pefile.PE(str(ROOT / "game" / exe), fast_load=True)
        self.pe.parse_data_directories(directories=[pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_IMPORT"]])
        self.image_end = self.image_base + self.pe.OPTIONAL_HEADER.SizeOfImage
        self.iat = {}
        for d in getattr(self.pe, "DIRECTORY_ENTRY_IMPORT", []):
            for imp in d.imports:
                nm = imp.name.decode() if imp.name else f"ord{imp.ordinal}"
                self.iat[imp.address] = f"{d.dll.decode().upper()}::{nm}"
        self.md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32)
        self._tokcache = {}
        self.callers = collections.defaultdict(list)
        for a, f in self.funcs.items():
            for c in f["callees"]:
                self.callers[c].append(a)
        self.features()

    def read(self, va, size):
        try:
            return self.pe.get_data(va - self.image_base, size)
        except Exception:
            return b""

    def tokens(self, f, size=None):
        entry = int(f["entry"], 16)
        size = size or f["size"]
        key = (entry, size)
        if key in self._tokcache:
            return self._tokcache[key]
        out = []
        for ins in self.md.disasm(self.read(entry, size), entry):
            ops = ins.op_str
            branch = BRANCH.match(ins.mnemonic) is not None

            def sub(m):
                v = int(m.group(0), 16)
                if branch and entry <= v < entry + size:
                    return "L%x" % (v - entry)
                if v in self.iat:
                    return self.iat[v]
                if self.image_base <= v < self.image_end:
                    return "A"
                return m.group(0)

            out.append(ins.mnemonic + " " + HEX.sub(sub, ops))
        self._tokcache[key] = out
        return out

    def features(self):
        for a, f in self.funcs.items():
            toks = self.tokens(f)
            f["toks"] = toks
            f["hash"] = hashlib.sha1("\n".join(toks).encode()).hexdigest() if toks else None
            strs = [self.strings.get(s, s) for s in f["strings"]]
            named = [r for r in f["data_refs"] if not r.startswith(("FUN_", "DAT_", "LAB_")) and r != "ExceptionList"]
            f["strkey"] = tuple(sorted(strs)) if strs else None
            f["strnamedkey"] = tuple(sorted(strs + named)) if (strs or named) else None
            f["extkey"] = tuple(sorted(f["calls_external"])) if f["calls_external"] else None
            f["fptrs"] = [("0x%08x" % int(r[4:], 16)) for r in f["data_refs"] if r.startswith("FUN_")]
            f["nstr"] = len(set(strs))

    def vtables(self):
        """{(class, col_offset): [method addresses]}"""
        out = {}
        for cls, info in self.rtti.items():
            for vt in info.get("vtables", []):
                out[(cls, vt.get("col_offset", 0))] = vt["methods"]
        return out


def sim(A, B, fa, fb):
    """Instruction similarity. Both sides are read for the larger of the two sizes, because Ghidra
    sometimes ends a function early (a shared tail or a jump target it made into a function)."""
    if fa["hash"] is not None and fa["hash"] == fb["hash"]:
        return 1.0
    n = max(fa["size"], fb["size"])
    ta, tb = A.tokens(fa, n), B.tokens(fb, n)
    if not ta or not tb:
        return 0.0
    sm = difflib.SequenceMatcher(None, ta, tb, autojunk=False)
    if sm.real_quick_ratio() < 0.5 or sm.quick_ratio() < 0.5:
        return sm.quick_ratio() * 0.9
    return sm.ratio()


class Matcher:
    def __init__(self, A, B):
        self.A, self.B = A, B
        self.ab, self.ba = {}, {}
        self.why = {}          # a -> first signal that proposed it
        self.conflicts = []    # (signal, a, b, reason)
        self._sim = {}

    def s(self, a, b):
        k = (a, b)
        if k not in self._sim:
            self._sim[k] = sim(self.A, self.B, self.A.funcs[a], self.B.funcs[b])
        return self._sim[k]

    def accept(self, a, b, why):
        if a in self.ab or b in self.ba:
            if self.ab.get(a) != b:
                self.conflicts.append((why, a, b, f"already {a}->{self.ab.get(a)} / {self.ba.get(b)}->{b}"))
            return False
        if self.A.tag.get(a) in ("eh",) or self.B.tag.get(b) in ("eh",):
            return False
        self.ab[a], self.ba[b] = b, a
        self.why[a] = why
        return True

    def unique_key(self, key, why, check=None):
        ga, gb = collections.defaultdict(list), collections.defaultdict(list)
        for a, f in self.A.funcs.items():
            if a not in self.ab and f[key] is not None and self.A.tag[a] != "eh":
                ga[f[key]].append(a)
        for b, f in self.B.funcs.items():
            if b not in self.ba and f[key] is not None and self.B.tag[b] != "eh":
                gb[f[key]].append(b)
        n = 0
        for k, la in ga.items():
            lb = gb.get(k, [])
            if len(la) == 1 and len(lb) == 1:
                a, b = la[0], lb[0]
                if check and not check(a, b):
                    self.conflicts.append((why, a, b, "rejected by size/similarity check"))
                    continue
                n += self.accept(a, b, why)
        return n

    def agree(self, a, b):
        """number of matched callers/callees of a whose counterparts are callers/callees of b"""
        cb = set(self.B.callers.get(b, []))
        n = sum(1 for c in set(self.A.callers.get(a, [])) if c in self.ab and self.ab[c] in cb)
        fb = set(self.B.funcs[b]["callees"])
        return n + sum(1 for c in set(self.A.funcs[a]["callees"]) if c in self.ab and self.ab[c] in fb)

    def small_ok(self, a, b):
        """a short function with a unique masked hash: needs the same imports or a matched neighbour"""
        fa, fb = self.A.funcs[a], self.B.funcs[b]
        return len(fa["toks"]) >= 3 and ((fa["extkey"] is not None and fa["extkey"] == fb["extkey"])
                                         or self.agree(a, b) >= 1)

    def near(self, a, k=5):
        """dst functions suggested for unmatched src a by its matched neighbours: (votes, sim, addr)"""
        A, B = self.A, self.B
        votes = collections.Counter()
        for c in set(A.callers.get(a, [])):
            if c in self.ab:
                for y in set(B.funcs[self.ab[c]]["callees"]):
                    if y in B.funcs and y not in self.ba and B.tag[y] != "eh":
                        votes[y] += 1
        for x in set(A.funcs[a]["callees"]):
            if x in self.ab:
                for y in set(B.callers.get(self.ab[x], [])):
                    if y not in self.ba and B.tag[y] != "eh":
                        votes[y] += 1
        out = [(v, round(self.s(a, y), 3), y) for y, v in votes.items()]
        out.sort(key=lambda t: (t[1] >= 0.5, t[0], t[1]), reverse=True)
        return out[:k]

    def near_round(self, min_sim=0.95, margin=0.1):
        """unmatched a whose matched neighbours point at one dst function that is byte-identical
        (read at the larger size) with a clear margin over the runner-up"""
        props = []
        for a, f in self.A.funcs.items():
            if a in self.ab or self.A.tag[a] in ("eh", "thunk") or len(f["toks"]) < 10:
                continue
            nb = self.near(a, k=50)
            nb = sorted(nb, key=lambda t: t[1], reverse=True)
            if not nb or nb[0][1] < min_sim:
                continue
            if len(nb) > 1 and nb[0][1] - nb[1][1] < margin:
                continue
            props.append((nb[0][1], a, nb[0][2]))
        cnt = collections.Counter(b for _, _, b in props)
        n = 0
        for sc, a, b in sorted(props, reverse=True):
            if cnt[b] == 1:
                n += self.accept(a, b, f"neighbourhood bytes {sc:.3f}")
        return n

    def size_ok(self, a, b, tol):
        sa, sb = self.A.funcs[a]["size"], self.B.funcs[b]["size"]
        return abs(sa - sb) <= tol * max(sa, sb) + 2

    def vtable_round(self):
        va, vb = self.A.vtables(), self.B.vtables()
        n = 0
        for k, ma in va.items():
            mb = vb.get(k)
            if not mb:
                continue
            for i, (a, b) in enumerate(zip(ma, mb)):
                if a in self.A.funcs and b in self.B.funcs:
                    if a not in self.ab and b not in self.ba and self.size_ok(a, b, 0.25):
                        n += self.accept(a, b, f"vtable {k[0]} slot {i}")
        return n

    def callee_round(self):
        n = 0
        for a, b in list(self.ab.items()):
            fa, fb = self.A.funcs[a], self.B.funcs[b]
            for lista, listb, kind in ((fa["callees"], fb["callees"], "callee"), (fa["fptrs"], fb["fptrs"], "fptr")):
                if len(lista) == len(listb):
                    pairs = list(zip(lista, listb))
                else:
                    ua = [x for x in dict.fromkeys(lista) if x not in self.ab and x in self.A.funcs]
                    ub = [y for y in dict.fromkeys(listb) if y not in self.ba and y in self.B.funcs]
                    pairs = list(zip(ua, ub)) if len(ua) == len(ub) == 1 else []
                for i, (x, y) in enumerate(pairs):
                    if x in self.ab or y in self.ba or x not in self.A.funcs or y not in self.B.funcs:
                        continue
                    if self.A.tag[x] == "eh" or self.B.tag[y] == "eh":
                        continue
                    if self.size_ok(x, y, 0.1) or self.s(x, y) >= 0.8:
                        n += self.accept(x, y, f"{kind} #{i} of matched {a}")
        return n

    def callee_set_round(self):
        """unmatched a whose callees are all matched: find unique unmatched b with that mapped callee set"""
        idx = collections.defaultdict(list)
        for b, f in self.B.funcs.items():
            if b not in self.ba and f["callees"]:
                idx[frozenset(f["callees"])].append(b)
        want = collections.defaultdict(list)
        for a, f in self.A.funcs.items():
            cs = set(f["callees"])
            if a in self.ab or len(cs) < 2 or not all(c in self.ab for c in cs):
                continue
            want[frozenset(self.ab[c] for c in cs)].append(a)
        n = 0
        for k, la in want.items():
            lb = idx.get(k, [])
            if len(la) == 1 and len(lb) == 1 and self.size_ok(la[0], lb[0], 0.15):
                n += self.accept(la[0], lb[0], "callee set matched")
        return n

    def fuzzy_round(self, min_size=64, min_sim=0.95):
        bs = sorted((f["size"], b) for b, f in self.B.funcs.items()
                    if b not in self.ba and f["size"] >= min_size and self.B.tag[b] not in ("eh", "thunk"))
        sizes = [s for s, _ in bs]
        import bisect
        props = []
        for a, f in self.A.funcs.items():
            if a in self.ab or f["size"] < min_size or self.A.tag[a] in ("eh", "thunk"):
                continue
            lo = bisect.bisect_left(sizes, f["size"] * 0.97)
            hi = bisect.bisect_right(sizes, f["size"] * 1.03 + 2)
            scored = sorted(((self.s(a, b), b) for _, b in bs[lo:hi]), reverse=True)
            if scored and scored[0][0] >= min_sim and (len(scored) == 1 or scored[0][0] - scored[1][0] >= 0.03):
                props.append((scored[0][0], a, scored[0][1]))
        n = 0
        # a b used by two a's is ambiguous
        cnt = collections.Counter(b for _, _, b in props)
        for sc, a, b in sorted(props, reverse=True):
            if cnt[b] == 1:
                n += self.accept(a, b, f"fuzzy bytes {sc:.3f}")
        return n

    def run(self):
        log = []
        def step(name, fn):
            k = fn()
            log.append((name, k))
            return k
        step("strings", lambda: self.unique_key("strkey", "strings"))
        step("strings+named refs", lambda: self.unique_key("strnamedkey", "strings+named refs"))
        step("imports+size", lambda: self.unique_key("extkey", "imports+size", lambda a, b: self.size_ok(a, b, 0.05)))
        step("vtable", self.vtable_round)
        for it in range(20):
            k = step(f"callgraph {it}", self.callee_round) + step(f"callee set {it}", self.callee_set_round)
            k += step(f"strings {it}", lambda: self.unique_key("strkey", "strings"))
            k += step(f"imports {it}", lambda: self.unique_key("extkey", "imports+size", lambda a, b: self.size_ok(a, b, 0.05)))
            if not k:
                break
        step("bytes exact", lambda: self.unique_key("hash", "bytes exact",
                                                    lambda a, b: len(self.A.funcs[a]["toks"]) >= 10))
        step("bytes exact small", lambda: self.unique_key("hash", "bytes exact small", self.small_ok))
        for it in range(20):
            k = step(f"callgraph b{it}", self.callee_round) + step(f"callee set b{it}", self.callee_set_round)
            k += step(f"bytes exact b{it}", lambda: self.unique_key("hash", "bytes exact",
                                                                  lambda a, b: len(self.A.funcs[a]["toks"]) >= 10))
            if not k:
                break
        step("fuzzy bytes", self.fuzzy_round)
        for it in range(20):
            k = step(f"callgraph f{it}", self.callee_round) + step(f"callee set f{it}", self.callee_set_round)
            k += step(f"neighbourhood {it}", self.near_round)
            if not k:
                break
        return log

    # ---- scoring
    def vt_slots(self):
        va, vb = self.A.vtables(), self.B.vtables()
        slots = collections.defaultdict(set)
        for k, ma in va.items():
            mb = vb.get(k, [])
            for i, (a, b) in enumerate(zip(ma, mb)):
                slots[(a, b)].add(f"{k[0].split('<')[0]} slot {i}")
        return slots

    def score(self, a, b, slots):
        fa, fb = self.A.funcs[a], self.B.funcs[b]
        ev, ps = [], []
        sm = self.s(a, b)
        strs_ok = fa["strkey"] is not None and fa["strkey"] == fb["strkey"]
        if strs_ok:
            p = 0.8 if fa["nstr"] >= 2 else 0.6
            ps.append(p); ev.append("strings")
        elif fa["strnamedkey"] is not None and fa["strnamedkey"] == fb["strnamedkey"]:
            ps.append(0.6); ev.append("named refs")
        if fa["extkey"] is not None and fa["extkey"] == fb["extkey"] and self.size_ok(a, b, 0.05):
            ps.append(0.5); ev.append("imports")
        if (a, b) in slots:
            ps.append(0.8); ev.append("vtable " + sorted(slots[(a, b)])[0])
        # callgraph agreement: matched callers of a that map to callers of b; matched callees agree
        cb = set(self.B.callers.get(b, []))
        agree_callers = sum(1 for c in set(self.A.callers.get(a, [])) if c in self.ab and self.ab[c] in cb)
        mc = [c for c in set(fa["callees"]) if c in self.ab]
        agree_callees = sum(1 for c in mc if self.ab[c] in set(fb["callees"]))
        nagree = agree_callers + agree_callees
        if nagree:
            ps.append(0.7 if nagree >= 2 else 0.5)
            ev.append(f"callgraph {agree_callers} callers/{agree_callees} callees")
        if sm >= 0.9999:
            ps.append(0.9); ev.append("bytes identical")
        elif sm >= 0.95:
            ps.append(0.7); ev.append(f"bytes {sm:.2f}")
        elif sm >= 0.8:
            ps.append(0.4); ev.append(f"bytes {sm:.2f}")
        else:
            ev.append(f"bytes {sm:.2f}")
        q = 1.0
        for p in ps:
            q *= 1 - p
        conf = 1 - q
        if sm < 0.5 and not strs_ok:
            conf *= 0.5
        return round(conf, 3), ev


def explain(m, a):
    """Print why src function a matched (a token diff) or which dst functions its neighbours suggest."""
    A, B = m.A, m.B
    fa = A.funcs.get(a)
    if fa is None:
        print(f"{a}: not a function in the {A.exe} export")
        return
    print(f"{a} size {fa['size']} tag {A.tag[a]} strings {fa['strkey']} ext {fa['extkey']}")
    if a in m.ab:
        b = m.ab[a]
        print(f"  matched {b} ({m.why[a]}) sim {m.s(a, b):.3f} size {B.funcs[b]['size']}")
        for line in list(difflib.unified_diff(fa["toks"], B.funcs[b]["toks"], lineterm="", n=1))[:60]:
            print("   ", line)
        return
    near = collections.Counter()
    for c in set(A.callers.get(a, [])):
        if c in m.ab:
            for y in set(B.funcs[m.ab[c]]["callees"]):
                if y in B.funcs and y not in m.ba:
                    near[y] += 1
    for x in set(fa["callees"]):
        if x in m.ab:
            for y in set(B.callers.get(m.ab[x], [])):
                if y not in m.ba:
                    near[y] += 1
    print(f"  unmatched; callers {sorted(set(A.callers.get(a, [])))[:8]} "
          f"(matched: {[m.ab.get(c) for c in set(A.callers.get(a, []))][:8]})")
    for y, k in near.most_common(12):
        fb = B.funcs[y]
        print(f"  near {y} votes {k} size {fb['size']} sim {m.s(a, y):.3f} tag {B.tag[y]} strings {fb['strkey']}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--src", default="Server.exe")
    ap.add_argument("--dst", default="Cube.exe")
    ap.add_argument("--threshold", type=float, default=0.9)
    ap.add_argument("--dump")
    ap.add_argument("--explain", nargs="*", help="print match detail for these src addresses and exit")
    args = ap.parse_args()

    A, B = Binary(args.src), Binary(args.dst)
    m = Matcher(A, B)
    for name, k in m.run():
        if k:
            print(f"  {name:28s} +{k}")
    if args.explain:
        for a in args.explain:
            explain(m, "0x%08x" % int(a, 16))
        return
    slots = m.vt_slots()
    scored = {}
    for a, b in m.ab.items():
        conf, ev = m.score(a, b, slots)
        scored[a] = (b, conf, ev)

    names = json.loads((ROOT / "analysis/names" / f"{args.src}.json").read_text())["names"]
    good, cands = {}, {}
    for a, rec in names.items():
        if a not in scored:
            cands[a] = {"name": rec["name"], "match": None,
                        "reason": "no match found" if a in A.funcs else "not a function in the export"}
            if a in A.funcs:
                cands[a]["near"] = [f"{y} (votes {v}, bytes {sv}, size {B.funcs[y]['size']} vs {A.funcs[a]['size']})"
                                    for v, sv, y in m.near(a)]
            continue
        b, conf, ev = scored[a]
        entry = {"name": rec["name"],
                 "evidence": f"matches {args.src} {a} ({' + '.join(ev)}; proposed by {m.why[a]}; conf {conf})"}
        if conf >= args.threshold:
            good["0x%08x" % int(b, 16)] = entry
        else:
            cands["0x%08x" % int(b, 16)] = dict(entry, confidence=conf, src=a)
    # a name carried twice onto one address, or two names onto one address, cannot happen (bijection)
    good = dict(sorted(good.items()))
    out = {"binary": args.dst,
           "note": (f"Names carried from {args.src} by tools/match_functions.py (Server.exe and Cube.exe are the "
                    f"same source built two minutes apart). Only matches with combined confidence >= "
                    f"{args.threshold} are here; doubtful ones are in {args.dst}.candidates.json. Evidence lists "
                    "every signal that holds for the pair: strings (same referenced strings), named refs, imports "
                    "(same external calls, size within 5%), vtable slot, callgraph (matched callers/callees "
                    "agree), bytes (instruction similarity after masking addresses)."),
           "names": good}
    (ROOT / "analysis/names" / f"{args.dst}.json").write_text(json.dumps(out, indent=1) + "\n")
    (ROOT / "analysis/names" / f"{args.dst}.candidates.json").write_text(json.dumps(
        {"binary": args.dst, "threshold": args.threshold,
         "note": "Names that could not be carried confidently. Keys are the Cube.exe candidate address "
                 "(or the source address when there was no candidate).",
         "candidates": cands,
         "conflicts": [{"signal": w, "src": a, "dst": b, "reason": r} for w, a, b, r in m.conflicts
                       if a in names and m.ab.get(a) != b]}, indent=1) + "\n")

    # stats
    print(f"pairs: {len(m.ab)}  confident: {sum(1 for v in scored.values() if v[1] >= args.threshold)}")
    print(f"named: {len(names)}  carried: {len(good)}  candidates/unmatched: {len(cands)}")
    tot = collections.Counter(A.tag[a] for a in A.funcs)
    hit = collections.Counter(A.tag[a] for a, v in scored.items() if v[1] >= args.threshold)
    anyhit = collections.Counter(A.tag[a] for a in scored)
    totb = collections.Counter(B.tag[b] for b in B.funcs)
    for t in sorted(tot, key=lambda t: -tot[t]):
        print(f"  {t:8s} src {tot[t]:5d}  matched {anyhit[t]:5d}  confident {hit[t]:5d}   dst total {totb[t]:5d}")
    if args.dump:
        Path(args.dump).write_text(json.dumps(
            {a: {"dst": b, "conf": c, "evidence": ev, "why": m.why[a], "tag_src": A.tag[a], "tag_dst": B.tag[b]}
             for a, (b, c, ev) in sorted(scored.items())}, indent=0))


if __name__ == "__main__":
    main()
