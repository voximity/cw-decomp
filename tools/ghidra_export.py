"""Dump a Ghidra program's functions, references, strings, and vtables to JSON.

Usage (the Ghidra MCP server must be stopped first; Ghidra allows one process per project):

    GHIDRA_INSTALL_DIR=/opt/homebrew/opt/ghidra/libexec \
    .venv/bin/python -P tools/ghidra_export.py game/Server.exe

Writes analysis/export/<binary>.json. Read-only: nothing in the project is modified.
"""
import json
import sys
import time
from pathlib import Path

import pyghidra

ROOT = Path(__file__).resolve().parent.parent
PROJECT_DIR = ROOT / "analysis" / "project"
OUT_DIR = ROOT / "analysis" / "export"


def export(exe: Path) -> Path:
    pyghidra.start()
    from ghidra.program.model.symbol import RefType, SourceType  # noqa: F401
    from ghidra.util.task import TaskMonitor

    with pyghidra.open_program(
        str(exe), project_location=str(PROJECT_DIR), project_name="cw",
        analyze=False, nested_project_location=False,
    ) as flat:
        prog = flat.getCurrentProgram()
        fm = prog.getFunctionManager()
        listing = prog.getListing()
        st = prog.getSymbolTable()
        rm = prog.getReferenceManager()
        mem = prog.getMemory()
        monitor = TaskMonitor.DUMMY

        t0 = time.time()
        funcs = {}
        for f in fm.getFunctions(True):
            entry = f.getEntryPoint().getOffset()
            funcs[entry] = {
                "entry": f"0x{entry:08x}",
                "name": f.getName(),
                "namespace": f.getParentNamespace().getName(True),
                "size": f.getBody().getNumAddresses(),
                "thunk": bool(f.isThunk()),
                "external": bool(f.isExternal()),
                "source": str(f.getSymbol().getSource()),
                "callees": [],
                "calls_external": [],
                "strings": [],
                "data_refs": [],
            }
        print(f"  {len(funcs)} functions collected in {time.time()-t0:.0f}s", flush=True)

        t0 = time.time()
        strings = {}
        n_refs = 0
        it = rm.getReferenceIterator(prog.getMinAddress())
        while it.hasNext():
            ref = it.next()
            n_refs += 1
            src = fm.getFunctionContaining(ref.getFromAddress())
            if src is None:
                continue
            rec = funcs[src.getEntryPoint().getOffset()]
            to = ref.getToAddress()
            rt = ref.getReferenceType()
            if to.isExternalAddress():
                sym = st.getPrimarySymbol(to)
                if sym is not None:
                    rec["calls_external"].append(sym.getName(True))
                continue
            if rt.isCall() or rt.isJump():
                tgt = fm.getFunctionAt(to)
                if tgt is not None and tgt.getEntryPoint() != src.getEntryPoint():
                    rec["callees"].append(f"0x{tgt.getEntryPoint().getOffset():08x}")
                elif tgt is None and rt.isCall():
                    # call into the middle of something, or a thunk target Ghidra missed
                    rec["callees"].append(f"0x{to.getOffset():08x}?")
                continue
            if rt.isData():
                data = listing.getDataContaining(to)
                if data is not None and data.hasStringValue():
                    key = f"0x{data.getAddress().getOffset():08x}"
                    if key not in strings:
                        strings[key] = str(data.getValue())
                    rec["strings"].append(key)
                else:
                    sym = st.getPrimarySymbol(to)
                    if sym is not None and not sym.isDynamic():
                        rec["data_refs"].append(sym.getName(True))
        print(f"  {n_refs} references walked in {time.time()-t0:.0f}s, {len(strings)} strings", flush=True)

        # RTTI classes and vtables. The RTTI analyser names vtables "vftable" under the class namespace.
        classes = {}
        for cls in st.getClassNamespaces():
            classes[cls.getName(True)] = {"vtables": []}
        vtables = []
        for sym in st.getSymbols("vftable"):
            ns = sym.getParentNamespace().getName(True)
            addr = sym.getAddress()
            entries = []
            a = addr
            while True:
                if a != addr and st.getPrimarySymbol(a) is not None:
                    break
                try:
                    ptr = mem.getInt(a) & 0xFFFFFFFF
                except Exception:
                    break
                tgt = fm.getFunctionAt(flat.toAddr(ptr))
                if tgt is None:
                    break
                entries.append(f"0x{ptr:08x}")
                a = a.add(4)
            vt = {"class": ns, "address": f"0x{addr.getOffset():08x}", "entries": entries}
            vtables.append(vt)
            classes.setdefault(ns, {"vtables": []})["vtables"].append(vt["address"])
        print(f"  {len(classes)} class namespaces, {len(vtables)} vtables", flush=True)

        imports = sorted({s.getName(True) for s in st.getExternalSymbols()})

        out = {
            "binary": exe.name,
            "md5": prog.getExecutableMD5(),
            "image_base": f"0x{prog.getImageBase().getOffset():08x}",
            "function_count": len(funcs),
            "functions": [funcs[k] for k in sorted(funcs)],
            "strings": strings,
            "classes": classes,
            "vtables": vtables,
            "imports": imports,
        }
        OUT_DIR.mkdir(parents=True, exist_ok=True)
        out_path = OUT_DIR / f"{exe.name}.json"
        out_path.write_text(json.dumps(out, indent=0))
        return out_path


if __name__ == "__main__":
    for arg in sys.argv[1:]:
        exe = (ROOT / arg).resolve() if not Path(arg).is_absolute() else Path(arg)
        print(f"== exporting {exe.name}", flush=True)
        p = export(exe)
        print(f"   wrote {p} ({p.stat().st_size/1e6:.1f} MB)", flush=True)
