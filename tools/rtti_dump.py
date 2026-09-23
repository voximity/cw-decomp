"""Recover the MSVC RTTI class hierarchy and vtables straight from a PE file.

Usage:
    .venv/bin/python tools/rtti_dump.py game/Server.exe game/Cube.exe

Writes analysis/rtti/<binary>.json (machine readable) and analysis/rtti/<binary>.md
(class tree). Independent of Ghidra, so it can run while the MCP server holds the project.

Layout (32-bit MSVC, absolute pointers):
  TypeDescriptor           { void* vftable; void* spare; char name[]; }      name starts ".?A"
  CompleteObjectLocator    { u32 sig=0; u32 offset; u32 cdOffset; TD* td; CHD* chd; }
  ClassHierarchyDescriptor { u32 sig; u32 attrs; u32 numBases; BCD** baseArray; }
  BaseClassDescriptor      { TD* td; u32 numContained; i32 mdisp, pdisp, vdisp; u32 attrs; }
  vftable[-1] is a pointer to the CompleteObjectLocator.
"""
import json
import re
import struct
import sys
from pathlib import Path

import pefile

ROOT = Path(__file__).resolve().parent.parent
OUT_DIR = ROOT / "analysis" / "rtti"


def demangle(td_name: str) -> str:
    # ".?AVCreature@cube@@" -> "cube::Creature"; templates are left as-is beyond the outer name.
    m = re.match(r"\.\?A[UV](.+)@@$", td_name)
    if not m:
        return td_name
    parts = [p for p in m.group(1).split("@") if p]
    return "::".join(reversed(parts))


class Image:
    def __init__(self, path: Path):
        self.pe = pefile.PE(str(path), fast_load=True)
        self.base = self.pe.OPTIONAL_HEADER.ImageBase
        self.data = self.pe.__data__
        self.sections = [(s.VirtualAddress + self.base, s.Misc_VirtualSize, s.PointerToRawData, s.SizeOfRawData, s.Name.rstrip(b"\0").decode())
                         for s in self.pe.sections]
        text = self.pe.sections[0]
        self.text_range = (self.base + text.VirtualAddress, self.base + text.VirtualAddress + text.Misc_VirtualSize)

    def off(self, va: int):
        for va0, vs, raw, rs, _ in self.sections:
            if va0 <= va < va0 + max(vs, rs):
                o = va - va0 + raw
                return o if o < len(self.data) else None
        return None

    def u32(self, va: int):
        o = self.off(va)
        if o is None or o + 4 > len(self.data):
            return None
        return struct.unpack_from("<I", self.data, o)[0]

    def cstr(self, va: int, limit=512):
        o = self.off(va)
        if o is None:
            return None
        end = self.data.find(b"\0", o, o + limit)
        return self.data[o:end].decode("latin1") if end != -1 else None

    def in_data_sections(self, va: int):
        return any(va0 <= va < va0 + vs for va0, vs, _, _, name in self.sections if name in (".rdata", ".data", "_RDATA"))

    def in_text(self, va: int):
        return self.text_range[0] <= va < self.text_range[1]


def dump(path: Path):
    img = Image(path)
    # 1. TypeDescriptors: every ".?A" string; the descriptor starts 8 bytes earlier.
    tds = {}
    for m in re.finditer(rb"\.\?A[UV][^\0]{1,400}\0", img.data):
        name = m.group()[:-1].decode("latin1")
        for va0, vs, raw, rs, _ in img.sections:
            if raw <= m.start() < raw + rs:
                td = va0 + (m.start() - raw) - 8
                tds[td] = name
                break
    # 2. CompleteObjectLocators: dwords in data sections whose value is a TD address,
    #    preceded by sig=0 and followed by a CHD pointer whose base array starts with this TD.
    cols = {}
    data_ranges = [(va0, vs, raw) for va0, vs, raw, rs, name in img.sections if name in (".rdata", ".data", "_RDATA")]
    for va0, vs, raw in data_ranges:
        for i in range(0, vs - 4, 4):
            va = va0 + i
            v = img.u32(va)
            if v not in tds:
                continue
            col = va - 12
            if img.u32(col) != 0:
                continue
            chd = img.u32(col + 16)
            if chd is None or not img.in_data_sections(chd):
                continue
            nbases = img.u32(chd + 8)
            bca = img.u32(chd + 12)
            if nbases is None or bca is None or nbases == 0 or nbases > 64 or not img.in_data_sections(bca):
                continue
            bcd0 = img.u32(bca)
            if bcd0 is None or img.u32(bcd0) != v:
                continue
            cols[col] = {"td": v, "offset": img.u32(col + 4), "chd": chd, "nbases": nbases, "bca": bca}
    # 3. vftables: dwords in data sections pointing at a COL; the vtable starts right after.
    col_refs = {}
    for va0, vs, raw in data_ranges:
        for i in range(0, vs - 4, 4):
            va = va0 + i
            v = img.u32(va)
            if v in cols:
                col_refs[va + 4] = v
    # Known vtable starts let us bound method counts.
    starts = sorted(col_refs)
    vtables = {}
    for vt in starts:
        methods = []
        a = vt
        while True:
            p = img.u32(a)
            if a != vt and (a in col_refs or p in cols):
                break
            if p is None or not img.in_text(p):
                break
            methods.append(f"0x{p:08x}")
            a += 4
        vtables[vt] = {"address": f"0x{vt:08x}", "col": f"0x{col_refs[vt]:08x}", "methods": methods}
    # 4. Classes: one per TD that owns at least one COL; hierarchy from the CHD base array.
    ghidra_names = {}
    export = ROOT / "analysis" / "export" / f"{path.name}.json"
    if export.exists():
        for v in json.loads(export.read_text())["vtables"]:
            ghidra_names[int(v["address"], 16)] = v["class"]
    td_names = {}
    for col, c in cols.items():
        for vt, colv in col_refs.items():
            if colv == col and vt in ghidra_names:
                td_names[c["td"]] = ghidra_names[vt]
    classes = {}
    for col, c in cols.items():
        vt_addrs = [vt for vt, colv in col_refs.items() if colv == col]
        name = next((ghidra_names[vt] for vt in vt_addrs if vt in ghidra_names), None) or demangle(tds[c["td"]])
        cls = classes.setdefault(name, {"mangled": tds[c["td"]], "td": f"0x{c['td']:08x}", "vtables": [], "bases": [], "all_bases": []})
        vts = [vtables[vt] | {"col_offset": c["offset"]} for vt, colv in col_refs.items() if colv == col]
        for v in vts:
            if v not in cls["vtables"]:
                cls["vtables"].append(v)
        if not cls["all_bases"]:
            arr = [img.u32(c["bca"] + 4 * i) for i in range(c["nbases"])]
            entries = []
            for bcd in arr:
                td = img.u32(bcd)
                entries.append({"name": td_names.get(td) or demangle(tds.get(td, f"td_{td:x}")), "contained": img.u32(bcd + 4),
                                "mdisp": struct.unpack("<i", struct.pack("<I", img.u32(bcd + 8)))[0]})
            cls["all_bases"] = [e["name"] for e in entries[1:]]
            direct = []
            i = 1
            while i < len(entries):
                direct.append(entries[i]["name"])
                i += entries[i]["contained"] + 1
            cls["bases"] = direct
    for cls in classes.values():
        cls["vtables"].sort(key=lambda v: v["col_offset"])
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    out = {"binary": path.name, "type_descriptors": len(tds), "classes": classes}
    (OUT_DIR / f"{path.name}.json").write_text(json.dumps(out, indent=1))
    # Markdown tree: roots are classes with no bases; children listed under each parent.
    children = {}
    for name, cls in classes.items():
        for b in cls["bases"]:
            children.setdefault(b, []).append(name)
    lines = [f"# RTTI class hierarchy: {path.name}", "",
             f"{len(classes)} classes with vtables, {len(tds)} type descriptors. Method counts are per primary vtable.", ""]

    def walk(name, depth, seen):
        cls = classes.get(name)
        n = len(cls["vtables"][0]["methods"]) if cls and cls["vtables"] else 0
        lines.append(f"{'  ' * depth}- `{name}` ({n} methods)" if cls else f"{'  ' * depth}- `{name}` (no vtable in this binary)")
        for ch in sorted(children.get(name, [])):
            if ch not in seen:
                seen.add(ch)
                walk(ch, depth + 1, seen)

    roots = sorted(n for n, c in classes.items() if not c["bases"])
    external_roots = sorted(b for c in classes.values() for b in c["bases"] if b not in classes)
    seen = set()
    for r in roots + sorted(set(external_roots)):
        if r not in seen:
            seen.add(r)
            walk(r, 0, seen)
    (OUT_DIR / f"{path.name}.md").write_text("\n".join(lines) + "\n")
    return len(tds), len(cols), len(vtables), len(classes)


if __name__ == "__main__":
    for arg in sys.argv[1:]:
        p = (ROOT / arg) if not Path(arg).is_absolute() else Path(arg)
        tds, cols, vts, classes = dump(p)
        print(f"{p.name}: {tds} type descriptors, {cols} object locators, {vts} vtables, {classes} classes")
