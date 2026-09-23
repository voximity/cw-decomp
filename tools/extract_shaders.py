"""Extract, parse and disassemble the Direct3D 9 shaders embedded in Cube.exe.

Usage:
    .venv/Scripts/python.exe tools/extract_shaders.py [game/Cube.exe]

Scans every PE section for D3D9 shader bytecode (a version token followed by comment and
instruction tokens up to the 0x0000FFFF END token), and writes to analysis/shaders/:

  <index>_<kind>_<rva>.bin   the raw bytecode, version token to END token inclusive
  <index>_<kind>_<rva>.asm   D3DDisassemble output (d3dcompiler_47.dll through ctypes), or
                             the minimal fallback disassembler below if that DLL fails
  manifest.json              offsets, sizes, CTAB constant table, dcl inputs/outputs,
                             samplers, def'd literal constants, instruction counts.

Creator functions (who references each bytecode address) are not found here; they come
from the Ghidra/Binary Ninja cross-references and are merged in from
analysis/shaders/creators.json when that file exists ({"<rva hex>": {...}}).

Token format (D3D9 "Shader Code Format" docs):
  version  0xFFFE0300 vs_3_0, 0xFFFF0300 ps_3_0, 0xFFFE0200 vs_2_0, 0xFFFF0200 ps_2_0
  comment  low 16 bits 0xFFFE, bits 16..30 = number of following DWORDs
  instr    low 16 bits opcode, bits 24..27 = number of following DWORDs (SM2+)
  end      0x0000FFFF
"""
import ctypes
import json
import struct
import sys
from pathlib import Path

import pefile

ROOT = Path(__file__).resolve().parent.parent
OUT_DIR = ROOT / "analysis" / "shaders"

VERSIONS = {
    0xFFFE0300: "vs_3_0",
    0xFFFF0300: "ps_3_0",
    0xFFFE0200: "vs_2_0",
    0xFFFF0200: "ps_2_0",
}
END = 0x0000FFFF
COMMENT = 0xFFFE

# ---------------------------------------------------------------------------------------
# Bytecode walking


def walk(data: bytes, off: int):
    """Walk tokens from a version token at `off`; return (end_off_exclusive, comments,
    instructions) or None when the stream is not a plausible shader."""
    n = len(data)
    pos = off + 4
    comments = []  # (dword offset, payload bytes)
    instrs = []  # (opcode, [param tokens])
    while pos + 4 <= n:
        tok = struct.unpack_from("<I", data, pos)[0]
        if tok == END:
            return pos + 4, comments, instrs
        op = tok & 0xFFFF
        if op == COMMENT:
            length = (tok >> 16) & 0x7FFF
            payload = data[pos + 4: pos + 4 + 4 * length]
            if len(payload) != 4 * length:
                return None
            comments.append(payload)
            pos += 4 + 4 * length
            continue
        if tok & 0x80000000:  # bit 31 must be clear on an instruction token
            return None
        if op > 0x60 and op not in (0xFFFD,):  # 0xFFFD = phase (ps_1_4); SM2/3 max is texldd etc.
            return None
        length = (tok >> 24) & 0x0F
        params = list(struct.unpack_from("<%dI" % length, data, pos + 4))
        instrs.append((op, tok, params))
        pos += 4 + 4 * length
        if len(instrs) > 4096:
            return None
    return None


# ---------------------------------------------------------------------------------------
# CTAB constant table (D3DXSHADER_CONSTANTTABLE, offsets relative to the byte after 'CTAB')

REGSETS = {0: "BOOL", 1: "INT4", 2: "FLOAT4", 3: "SAMPLER"}
CLASSES = {0: "SCALAR", 1: "VECTOR", 2: "MATRIX_ROWS", 3: "MATRIX_COLUMNS", 4: "OBJECT", 5: "STRUCT"}
TYPES = {0: "VOID", 1: "BOOL", 2: "INT", 3: "FLOAT", 4: "STRING", 5: "TEXTURE", 6: "TEXTURE1D",
         7: "TEXTURE2D", 8: "TEXTURE3D", 9: "TEXTURECUBE", 10: "SAMPLER", 11: "SAMPLER1D",
         12: "SAMPLER2D", 13: "SAMPLER3D", 14: "SAMPLERCUBE"}
REGPREFIX = {0: "b", 1: "i", 2: "c", 3: "s"}


def cstr(buf: bytes, off: int) -> str:
    if off <= 0 or off >= len(buf):
        return ""
    end = buf.find(b"\0", off)
    return buf[off:end if end >= 0 else len(buf)].decode("latin-1")


def parse_ctab(payload: bytes):
    if payload[:4] != b"CTAB":
        return None
    t = payload[4:]
    size, creator, version, count, cinfo, flags, target = struct.unpack_from("<7I", t, 0)
    consts = []
    for i in range(count):
        name, regset, regidx, regcnt, _res, tinfo, defval = struct.unpack_from(
            "<IHHHHII", t, cinfo + 20 * i)
        cls, typ, rows, cols, elems, members, _minfo = struct.unpack_from("<6HI", t, tinfo)
        entry = {
            "name": cstr(t, name),
            "register": "%s%d" % (REGPREFIX.get(regset, "?"), regidx),
            "register_set": REGSETS.get(regset, str(regset)),
            "register_index": regidx,
            "register_count": regcnt,
            "class": CLASSES.get(cls, str(cls)),
            "type": TYPES.get(typ, str(typ)),
            "rows": rows,
            "columns": cols,
            "elements": elems,
        }
        if members:
            entry["struct_members"] = members
        if defval:
            nfl = max(1, regcnt) * 4
            entry["default"] = list(struct.unpack_from("<%df" % nfl, t, defval))
        consts.append(entry)
    return {
        "creator": cstr(t, creator),
        "target": cstr(t, target),
        "version": "0x%08X" % version,
        "flags": "0x%X" % flags,
        "constants": consts,
    }


# ---------------------------------------------------------------------------------------
# D3DDisassemble through ctypes


class _Blob:
    def __init__(self):
        self.dll = None
        try:
            self.dll = ctypes.WinDLL(r"C:\Windows\System32\d3dcompiler_47.dll")
            self.fn = self.dll.D3DDisassemble
            self.fn.restype = ctypes.c_long
            self.fn.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_uint,
                                ctypes.c_char_p, ctypes.POINTER(ctypes.c_void_p)]
        except (OSError, AttributeError) as e:
            print("d3dcompiler_47 unavailable:", e)
            self.dll = None

    def disassemble(self, code: bytes):
        if self.dll is None:
            return None
        buf = ctypes.create_string_buffer(code, len(code))
        out = ctypes.c_void_p()
        hr = self.fn(buf, len(code), 0, None, ctypes.byref(out))
        if hr < 0 or not out.value:
            print("D3DDisassemble failed hr=0x%08X" % (hr & 0xFFFFFFFF))
            return None
        # ID3DBlob vtable: QueryInterface, AddRef, Release, GetBufferPointer, GetBufferSize
        vtbl = ctypes.cast(ctypes.c_void_p.from_address(out.value).value,
                           ctypes.POINTER(ctypes.c_void_p))
        proto_ptr = ctypes.WINFUNCTYPE(ctypes.c_void_p, ctypes.c_void_p)
        proto_size = ctypes.WINFUNCTYPE(ctypes.c_size_t, ctypes.c_void_p)
        proto_rel = ctypes.WINFUNCTYPE(ctypes.c_ulong, ctypes.c_void_p)
        ptr = proto_ptr(vtbl[3])(out.value)
        size = proto_size(vtbl[4])(out.value)
        text = ctypes.string_at(ptr, size).rstrip(b"\0").decode("latin-1")
        proto_rel(vtbl[2])(out.value)
        return text


# ---------------------------------------------------------------------------------------
# Minimal fallback disassembler (only used if D3DDisassemble fails). Opcode table from the
# D3D9 "Instruction Token" docs (D3DSHADER_INSTRUCTION_OPCODE_TYPE).

OPCODES = {
    0: "nop", 1: "mov", 2: "add", 3: "sub", 4: "mad", 5: "mul", 6: "rcp", 7: "rsq", 8: "dp3",
    9: "dp4", 10: "min", 11: "max", 12: "slt", 13: "sge", 14: "exp", 15: "log", 16: "lit",
    17: "dst", 18: "lrp", 19: "frc", 20: "m4x4", 21: "m4x3", 22: "m3x4", 23: "m3x3",
    24: "m3x2", 25: "call", 26: "callnz", 27: "loop", 28: "ret", 29: "endloop", 30: "label",
    31: "dcl", 32: "pow", 33: "crs", 34: "sgn", 35: "abs", 36: "nrm", 37: "sincos",
    38: "rep", 39: "endrep", 40: "if", 41: "ifc", 42: "else", 43: "endif", 44: "break",
    45: "breakc", 46: "mova", 47: "defb", 48: "defi", 64: "texcoord", 65: "texkill",
    66: "texld", 67: "texbem", 68: "texbeml", 69: "texreg2ar", 70: "texreg2gb",
    71: "texm3x2pad", 72: "texm3x2tex", 73: "texm3x3pad", 74: "texm3x3tex",
    76: "texm3x3spec", 77: "texm3x3vspec", 78: "expp", 79: "logp", 80: "cnd", 81: "def",
    82: "texreg2rgb", 83: "texdp3tex", 84: "texm3x2depth", 85: "texdp3", 86: "texm3x3",
    87: "texdepth", 88: "cmp", 89: "bem", 90: "dp2add", 91: "dsx", 92: "dsy", 93: "texldd",
    94: "setp", 95: "texldl", 96: "breakp",
}
REGTYPES = {0: "r", 1: "v", 2: "c", 3: "a", 4: "rast", 5: "attr", 6: "o", 7: "i", 8: "oC",
            9: "oDepth", 10: "s", 11: "c", 12: "c", 13: "c", 14: "b", 15: "aL", 16: "vFace",
            17: "misc", 18: "label", 19: "p"}
USAGES = {0: "position", 1: "blendweight", 2: "blendindices", 3: "normal", 4: "psize",
          5: "texcoord", 6: "tangent", 7: "binormal", 8: "tessfactor", 9: "positiont",
          10: "color", 11: "fog", 12: "depth", 13: "sample"}
TEXTYPES = {2: "2d", 3: "cube", 4: "volume"}


def regname(tok: int, vs: bool) -> str:
    rtype = ((tok >> 28) & 7) | ((tok >> 8) & 0x18)
    num = tok & 0x7FF
    if rtype == 4:  # rast out
        return ["oPos", "oFog", "oPts"][num] if num < 3 else "rast%d" % num
    if rtype == 5 and vs:
        return "oD%d" % num
    if rtype == 6 and vs:
        return "o%d" % num
    if rtype == 3 and not vs:
        return "t%d" % num
    return "%s%d" % (REGTYPES.get(rtype, "?"), num)


CMPS = {1: "gt", 2: "eq", 3: "ge", 4: "lt", 5: "ne", 6: "le"}


def src_operand(p: int, vs: bool) -> str:
    sw = (p >> 16) & 0xFF
    comps = "".join("xyzw"[(sw >> (2 * k)) & 3] for k in range(4))
    if comps == "xyzw":
        comps = ""
    elif len(set(comps)) == 1:
        comps = comps[0]
    r = regname(p, vs)
    mod = (p >> 24) & 0xF
    if mod in (11, 12):
        r += "_abs"
    if mod in (1, 12):
        r = "-" + r
    return r + ("." + comps if comps else "")


def fallback_disasm(kind: str, instrs) -> str:
    vs = kind.startswith("vs")
    lines = ["    " + kind]
    for op, tok, params in instrs:
        name = OPCODES.get(op, "op%d" % op)
        if op == 31:  # dcl
            usage = params[0]
            reg = regname(params[1], vs)
            if (params[1] >> 28) & 7 == 2 or regname(params[1], vs).startswith("s"):
                name = "dcl_" + TEXTYPES.get((usage >> 27) & 0xF, "?")
            else:
                u = USAGES.get(usage & 0x1F, "u%d" % (usage & 0x1F))
                idx = (usage >> 16) & 0xF
                name = "dcl_%s%s" % (u, idx if idx else "")
            lines.append("    %s %s" % (name, reg))
            continue
        if op == 81:
            vals = struct.unpack("<4f", struct.pack("<4I", *params[1:5]))
            lines.append("    def %s, %s" % (regname(params[0], vs), ", ".join("%g" % v for v in vals)))
            continue
        # Flow-control opcodes take only source operands; everything else starts with a dest.
        srcs_only = op in (25, 26, 27, 38, 40, 41, 45, 96)
        comp = (tok >> 16) & 7
        if op in (41, 45, 94) and comp:
            name += "_" + CMPS.get(comp, str(comp))
        ops = []
        for i, p in enumerate(params):
            if i == 0 and not srcs_only:
                mask = (p >> 16) & 0xF
                mod = (p >> 20) & 0xF
                if mod & 1:
                    name += "_sat"
                if mod & 2:
                    name += "_pp"
                m = "" if mask == 0xF else "." + "".join("xyzw"[k] for k in range(4) if mask >> k & 1)
                ops.append(regname(p, vs) + m)
            else:
                ops.append(src_operand(p, vs))
        lines.append("    %s %s" % (name, ", ".join(ops)))
    return "\n".join(lines) + "\n"


# ---------------------------------------------------------------------------------------


def analyse_asm(asm: str):
    """Pull dcl lines, def constants, register use and slot count out of the disassembly."""
    inputs, outputs, samplers, defs, used_c = [], [], [], [], set()
    slots = None
    count = 0
    for raw in asm.splitlines():
        line = raw.strip()
        if line.startswith("// approximately"):
            try:
                slots = int(line.split()[2])
            except ValueError:
                pass
        if not line or line.startswith("//") or line.startswith(("vs_", "ps_")):
            continue
        tok = line.split()
        if tok[0].startswith("dcl"):
            reg = tok[1].rstrip(",")
            sem = tok[0][4:]
            if not sem and reg[0] in "tv":  # ps_2_0 'dcl t0' / 'dcl v0': implicit semantics
                sem = ("texcoord" if reg[0] == "t" else "color") + (reg[1:].split(".")[0].lstrip("0") or "")
                tok[0] = "dcl_" + sem
            if reg.startswith("s"):
                samplers.append({"register": reg, "type": tok[0][4:]})
            elif reg.startswith("o"):
                outputs.append({"semantic": tok[0][4:], "register": reg})
            else:
                inputs.append({"semantic": tok[0][4:], "register": reg})
            continue
        if tok[0] == "def":
            defs.append(line[4:])
            continue
        count += 1
        for t in line.replace(",", " ").split()[1:]:
            t = t.lstrip("-").split(".")[0].split("[")[0]
            if t.startswith("c") and t[1:].isdigit():
                used_c.add(int(t[1:]))
    return {
        "inputs": inputs,
        "outputs": outputs,
        "samplers": samplers,
        "literal_constants": defs,
        "instructions": count,
        "instruction_slots": slots,
        "float_registers_read": sorted(used_c),
    }


def main():
    exe = Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT / "game" / "Cube.exe"
    pe = pefile.PE(str(exe), fast_load=True)
    base = pe.OPTIONAL_HEADER.ImageBase
    raw = pe.__data__
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    for old in list(OUT_DIR.glob("*.bin")) + list(OUT_DIR.glob("*.asm")):
        old.unlink()
    creators_path = OUT_DIR / "creators.json"
    creators = json.loads(creators_path.read_text()) if creators_path.exists() else {}

    dis = _Blob()
    found = []
    for sec in pe.sections:
        sname = sec.Name.rstrip(b"\0").decode()
        data = sec.get_data()
        pos = 0
        while True:
            hits = [data.find(struct.pack("<I", v), pos) for v in VERSIONS]
            hits = [h for h in hits if h >= 0]
            if not hits:
                break
            h = min(hits)
            pos = h + 1
            ver = struct.unpack_from("<I", data, h)[0]
            res = walk(data, h)
            if res is None:
                continue
            end, comments, instrs = res
            if not instrs:
                continue
            found.append((sname, sec, h, end, ver, comments, instrs))
            pos = end

    manifest = []
    for index, (sname, sec, h, end, ver, comments, instrs) in enumerate(found):
        kind = VERSIONS[ver]
        rva = sec.VirtualAddress + h
        fileoff = sec.PointerToRawData + h
        code = raw[fileoff: fileoff + (end - h)]
        stem = "%02d_%s_%06x" % (index, kind, rva)
        (OUT_DIR / (stem + ".bin")).write_bytes(code)
        asm = dis.disassemble(code)
        disassembler = "d3dcompiler_47 D3DDisassemble"
        if asm is None:
            asm = fallback_disasm(kind, instrs)
            disassembler = "fallback (tools/extract_shaders.py)"
        (OUT_DIR / (stem + ".asm")).write_text(asm, newline="\n")
        ctab = None
        for c in comments:
            ctab = parse_ctab(c) or ctab
        entry = {
            "index": index,
            "file": stem + ".bin",
            "kind": kind,
            "section": sname,
            "file_offset": "0x%x" % fileoff,
            "rva": "0x%x" % rva,
            "va": "0x%08x" % (base + rva),
            "size": end - h,
            "tokens": (end - h) // 4,
            "disassembler": disassembler,
            "sha256_prefix": __import__("hashlib").sha256(code).hexdigest()[:16],
        }
        entry.update(analyse_asm(asm))
        if ctab:
            entry["ctab"] = ctab
        extra = creators.get("0x%08x" % (base + rva))
        if extra:
            entry.update(extra)
        manifest.append(entry)
        print("%s  %s va=%s size=%d instr=%d consts=%d" % (
            stem, sname, entry["va"], entry["size"], entry["instructions"],
            len(ctab["constants"]) if ctab else 0))

    (OUT_DIR / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", newline="\n")
    print("%d shaders" % len(manifest))


if __name__ == "__main__":
    main()
