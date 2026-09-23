"""Apply classification tags from analysis/tags/<binary>.json to the Ghidra project as function tags.

Usage (Ghidra MCP server must be stopped):

    GHIDRA_INSTALL_DIR=/opt/homebrew/opt/ghidra/libexec \
    .venv/bin/python -P tools/ghidra_apply_tags.py game/Server.exe game/Cube.exe

Each function gets one classification tag (cube, plasma, abstr, sqlite, freetype, crt, ppl,
eh, thunk, unknown) and, when the classifier inferred it rather than proved it, the extra tag
"derived". Re-running replaces the previous classification tags. Filter by tag in Ghidra's
Functions window, or in a script via Function.getTags().
"""
import json
import sys
import time
from pathlib import Path

import pyghidra

ROOT = Path(__file__).resolve().parent.parent
PROJECT_DIR = ROOT / "analysis" / "project"
CLASS_TAGS = {"cube", "plasma", "abstr", "sqlite", "freetype", "crt", "ppl", "eh", "thunk", "unknown"}
ALL_TAGS = CLASS_TAGS | {"derived"}


def apply(exe: Path):
    tags = json.loads((ROOT / "analysis" / "tags" / f"{exe.name}.json").read_text())["tags"]
    pyghidra.start()
    with pyghidra.open_program(
        str(exe), project_location=str(PROJECT_DIR), project_name="cw",
        analyze=False, nested_project_location=False,
    ) as flat:
        prog = flat.getCurrentProgram()
        fm = prog.getFunctionManager()
        t0 = time.time()
        applied = 0
        missing = 0
        with pyghidra.transaction(prog, "apply classification tags"):
            for entry, t in tags.items():
                f = fm.getFunctionAt(flat.toAddr(int(entry, 16)))
                if f is None:
                    missing += 1
                    continue
                for old in list(f.getTags()):
                    if old.getName() in ALL_TAGS:
                        f.removeTag(old.getName())
                f.addTag(t["tag"])
                if t["confidence"] == "derived":
                    f.addTag("derived")
                applied += 1
        # open_program saves the program on exit; an explicit save here collides with it.
        print(f"  {exe.name}: tagged {applied} functions, {missing} not found, {time.time()-t0:.0f}s", flush=True)


if __name__ == "__main__":
    for arg in sys.argv[1:]:
        p = (ROOT / arg) if not Path(arg).is_absolute() else Path(arg)
        apply(p)
