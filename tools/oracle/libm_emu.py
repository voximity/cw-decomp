r"""Golden samples for the MSVCR110 math routines Server.exe calls, by CPU emulation.

Server.exe (2013-07-23) does not link the C runtime statically: its math calls go through the
import table to `game/msvcr110.dll` (`_libm_sse2_{sin,cos,asin,acos,exp,pow,sqrt}_precise`;
no `_CI*`, `atan2`, `log` or `fmod` import exists). This script maps that DLL's image at its
preferred base (0x10000000, so no relocation is needed) into the unicorn x86-32 emulator and
calls the exports directly, with the argument(s) in xmm0 (xmm1 for `pow`'s exponent) and the
result read back from xmm0, under the default MXCSR (0x1f80) and x87 control word (0x027f) the
game runs with. Server.exe itself is never run.

The only runtime service the routines reach is the math error handler (RVA 0xc09c8), which
calls the default `_matherr` (returns 0) and then `_errno()` to store EDOM/ERANGE; `_errno`
(RVA 0x11bbc) needs the thread data block, so it is replaced by a stub returning a scratch
address. Any other escape (an import call, an unmapped access) aborts the sample.

    .venv\Scripts\python.exe tools/oracle/libm_emu.py selftest
    .venv\Scripts\python.exe tools/oracle/libm_emu.py golden --out crates/cw-math/tests/golden
    .venv\Scripts\python.exe tools/oracle/libm_emu.py hash pow --count 200000
    .venv\Scripts\python.exe tools/oracle/libm_emu.py eval pow 2.0 0.5

Golden files: one sample per line, little-endian hex of the f64 argument(s) then the f64
result, the layout of `crates/cw-math/tests/golden/cos_msvcr110.txt`.
"""
import argparse
import math
import random
import struct
import sys
from pathlib import Path

import pefile
from unicorn import Uc, UcError, UC_ARCH_X86, UC_MODE_32, UC_HOOK_CODE, UC_HOOK_MEM_UNMAPPED
from unicorn.x86_const import (
    UC_X86_REG_EAX, UC_X86_REG_EIP, UC_X86_REG_ESP, UC_X86_REG_FPCW, UC_X86_REG_MXCSR,
    UC_X86_REG_XMM0, UC_X86_REG_XMM1,
)

ROOT = Path(__file__).resolve().parent.parent.parent
DLL = ROOT / "game" / "msvcr110.dll"

STACK_TOP = 0x0020_0000
STACK_SIZE = 0x0001_0000
RET_MAGIC = 0x0030_0000  # return address pushed for the call; emulation stops there
SCRATCH = 0x0031_0000  # errno lives here
ERRNO_RVA = 0x11BBC

# Exports and their arity (all take and return doubles in xmm registers).
ROUTINES = {
    "cos": ("_libm_sse2_cos_precise", 1),
    "sin": ("_libm_sse2_sin_precise", 1),
    "sqrt": ("_libm_sse2_sqrt_precise", 1),
    "asin": ("_libm_sse2_asin_precise", 1),
    "acos": ("_libm_sse2_acos_precise", 1),
    "exp": ("_libm_sse2_exp_precise", 1),
    "pow": ("_libm_sse2_pow_precise", 2),
}


def f2b(x: float) -> int:
    return struct.unpack("<Q", struct.pack("<d", x))[0]


def b2f(b: int) -> float:
    return struct.unpack("<d", struct.pack("<Q", b & (2**64 - 1)))[0]


def hexle(b: int) -> str:
    return struct.pack("<Q", b).hex()


class Libm:
    def __init__(self):
        pe = pefile.PE(str(DLL))
        self.base = pe.OPTIONAL_HEADER.ImageBase
        image = pe.get_memory_mapped_image()
        size = (len(image) + 0xFFF) & ~0xFFF
        self.exports = {e.name.decode(): self.base + e.address
                        for e in pe.DIRECTORY_ENTRY_EXPORT.symbols if e.name}
        uc = Uc(UC_ARCH_X86, UC_MODE_32)
        uc.mem_map(self.base, size)
        uc.mem_write(self.base, image)
        uc.mem_map(STACK_TOP - STACK_SIZE, STACK_SIZE)
        uc.mem_map(RET_MAGIC, 0x1000)
        uc.mem_write(RET_MAGIC, b"\xf4")  # hlt, never executed (emu stops at `until`)
        uc.mem_map(SCRATCH, 0x1000)
        errno_addr = self.base + ERRNO_RVA

        def on_code(uc, address, size, _):
            if address == errno_addr:  # _errno(): return &scratch, skip the body
                esp = uc.reg_read(UC_X86_REG_ESP)
                ret = struct.unpack("<I", uc.mem_read(esp, 4))[0]
                uc.reg_write(UC_X86_REG_EAX, SCRATCH)
                uc.reg_write(UC_X86_REG_ESP, esp + 4)
                uc.reg_write(UC_X86_REG_EIP, ret)

        def on_unmapped(uc, access, address, size, value, _):
            raise RuntimeError(f"unmapped access {access} at {address:#x}")

        uc.hook_add(UC_HOOK_CODE, on_code, begin=errno_addr, end=errno_addr)
        uc.hook_add(UC_HOOK_MEM_UNMAPPED, on_unmapped)
        self.uc = uc

    def call_bits(self, name: str, *args: int) -> int:
        export, arity = ROUTINES[name]
        assert len(args) == arity
        uc = self.uc
        esp = STACK_TOP - 0x100
        uc.mem_write(esp, struct.pack("<I", RET_MAGIC))
        uc.reg_write(UC_X86_REG_ESP, esp)
        uc.reg_write(UC_X86_REG_MXCSR, 0x1F80)
        uc.reg_write(UC_X86_REG_FPCW, 0x027F)
        uc.reg_write(UC_X86_REG_XMM0, args[0])
        uc.reg_write(UC_X86_REG_XMM1, args[1] if arity > 1 else 0)
        uc.emu_start(self.exports[export], RET_MAGIC, count=200_000)
        if uc.reg_read(UC_X86_REG_EIP) != RET_MAGIC:
            raise RuntimeError(f"{name} did not return")
        r = uc.reg_read(UC_X86_REG_XMM0) & (2**64 - 1)
        if (r >> 52) & 0x7FF == 0x7FF and r & 0x000F_FFFF_FFFF_FFFF and not r & (1 << 51):
            # A signaling NaN can only come back through the error handler's x87 `fld`/`fstp`
            # of an argument (SSE arithmetic always quiets). Real x87 hardware quiets an SNaN
            # on `fld qword`; QEMU's softfloat (unicorn) keeps it signaling. Model the hardware.
            r |= 1 << 51
        return r

    def call(self, name: str, *args: float) -> float:
        return b2f(self.call_bits(name, *(f2b(a) for a in args)))


# ----------------------------------------------------------------------------------------------
# Input sampling
# ----------------------------------------------------------------------------------------------

SPECIALS = [0.0, -0.0, 5e-324, -5e-324, 2.2250738585072014e-308, -2.2250738585072014e-308,
            1e-310, -1e-310, 1e-300, 1e-20, 1e-8, -1e-8, 0.5, -0.5, 1.0, -1.0, 2.0, -2.0,
            math.inf, -math.inf, math.nan, b2f(0xFFF8000000000000), b2f(0x7FF4000000000000),
            1.7976931348623157e308, -1.7976931348623157e308]


def f32(x: float) -> float:
    return struct.unpack("<f", struct.pack("<f", x))[0]


def random_bits(rng: random.Random) -> float:
    return b2f(rng.getrandbits(64))


def inputs_unit(rng: random.Random, n: int) -> list:
    """asin/acos: dense in [-1, 1] (as floats and as doubles), plus the edges around +-1."""
    xs = list(SPECIALS)
    one = f2b(1.0)
    for k in range(1, 40):
        xs += [b2f(one - k), -b2f(one - k), b2f(one + k), -b2f(one + k)]
    for _ in range(n):
        u = rng.uniform(-1.0, 1.0)
        xs += [u, f32(u)]
        e = rng.randint(-60, 0)
        xs.append(math.copysign(rng.random() * 2.0 ** e, rng.random() - 0.5))
    for _ in range(n // 10):
        xs.append(random_bits(rng))
    return xs


def inputs_exp(rng: random.Random, n: int) -> list:
    xs = list(SPECIALS)
    for v in (709.78, 709.7827128933840, 709.79, 710.0, -708.39, -708.4, -744.44, -745.13,
              -745.14, -746.0, 1e-17, -1e-17, 88.72, -87.33, -103.97):
        xs += [v, math.nextafter(v, math.inf), math.nextafter(v, -math.inf)]
    for _ in range(n):
        xs += [rng.uniform(-20.0, 20.0), f32(rng.uniform(-100.0, 100.0)),
               rng.uniform(-750.0, 712.0)]
        e = rng.randint(-60, 0)
        xs.append(math.copysign(rng.random() * 2.0 ** e, rng.random() - 0.5))
    for _ in range(n // 10):
        xs.append(random_bits(rng))
    return xs


def inputs_pow(rng: random.Random, n: int) -> list:
    pairs = []
    bases = [1.5, 2.0, 0.9, 1.1, 1.05, 0.95, 0.5, 3.0, 10.0, f32(1.1), f32(0.9), f32(1.05)]
    for b in bases + [-b for b in bases]:
        for k in range(0, 9):
            pairs += [(b, float(k)), (b, -float(k))]
    for x in SPECIALS:
        for y in SPECIALS + [0.5, 3.0, -3.0, 2.5, 1e300]:
            pairs.append((x, y))
    for _ in range(n):
        # Game-shaped: float base in (0, 4], float exponent in [-10, 100].
        pairs.append((f32(rng.uniform(0.0, 4.0)), f32(rng.uniform(-10.0, 100.0))))
        pairs.append((rng.choice(bases), f32(rng.uniform(0.0, 200.0))))
        pairs.append((rng.uniform(0.0, 10.0), rng.uniform(-30.0, 30.0)))
        pairs.append((rng.uniform(-5.0, 5.0), float(rng.randint(-40, 40))))
        pairs.append((b2f(rng.getrandbits(63)), rng.uniform(-3.0, 3.0)))
    for _ in range(n // 10):
        pairs.append((random_bits(rng), random_bits(rng)))
    return pairs


# ----------------------------------------------------------------------------------------------


def write_golden(path: Path, name: str, rows):
    with open(path, "w", newline="\n") as f:
        args = "x_f64hex y_f64hex" if name == "pow" else "x_f64hex"
        f.write(f"# MSVCR110 {name} (msvcr110.dll {ROUTINES[name][0]}, unicorn emulation by "
                f"tools/oracle/libm_emu.py): {args} result_f64hex (little endian)\n")
        for r in rows:
            f.write(" ".join(hexle(b) for b in r) + "\n")


def cmd_golden(args):
    lib = Libm()
    out = Path(args.out)
    rng = random.Random(args.seed)
    for name, gen in (("asin", inputs_unit), ("acos", inputs_unit), ("exp", inputs_exp)):
        rows = []
        for x in gen(rng, args.count):
            xb = f2b(x)
            rows.append((xb, lib.call_bits(name, xb)))
        write_golden(out / f"{name}_msvcr110.txt", name, rows)
        print(name, len(rows))
    rows = []
    for x, y in inputs_pow(rng, args.count):
        xb, yb = f2b(x), f2b(y)
        rows.append((xb, yb, lib.call_bits("pow", xb, yb)))
    write_golden(out / "pow_msvcr110.txt", "pow", rows)
    print("pow", len(rows))


def read_pairs(path: Path):
    for line in path.read_text().splitlines():
        if line.startswith("#") or not line.strip():
            continue
        yield [struct.unpack("<Q", bytes.fromhex(t))[0] for t in line.split()]


def cmd_selftest(args):
    """Replays the cos/sin goldens captured from the running Server.exe through the emulator."""
    lib = Libm()
    golden = ROOT / "crates" / "cw-math" / "tests" / "golden"
    for name, file in (("cos", "cos_msvcr110.txt"), ("sin", "sin_msvcr110.txt")):
        n = bad = 0
        for x, want in read_pairs(golden / file):
            got = lib.call_bits(name, x)
            n += 1
            if got != want:
                bad += 1
                if bad <= 5:
                    print(f"{name}({b2f(x)!r}) = {b2f(got)!r}, original {b2f(want)!r}")
            if n >= args.limit:
                break
        print(f"{name}: {n - bad}/{n} match")


def lcg32(i: int) -> int:
    return (i * 0x9E3779B1) & 0xFFFFFFFF


def hash_inputs(name: str, i: int):
    """The deterministic input sequence of `cmd_hash`, mirrored in cw-math's golden tests."""
    a = lcg32(2 * i) / 4294967296.0
    b = lcg32(2 * i + 1) / 4294967296.0
    if name in ("asin", "acos"):
        return (a * 2.0 - 1.0,)
    if name == "exp":
        return (a * 1500.0 - 750.0,)
    if name == "pow":
        return (a * 4.0, b * 400.0 - 200.0)
    if name == "pow_wide":  # |y log2 x| up to ~1600: overflow, subnormal results, underflow
        return (a * 1e6, b * 160.0 - 80.0)
    raise ValueError(name)


def cmd_hash(args):
    """FNV-1a 64 over the little-endian results for `hash_inputs(name, 0..count)`."""
    lib = Libm()
    h = 0xCBF29CE484222325
    for i in range(args.count):
        r = lib.call_bits(args.name.split("_")[0], *(f2b(v) for v in hash_inputs(args.name, i)))
        for byte in struct.pack("<Q", r):
            h = ((h ^ byte) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    print(f"{args.name} {args.count} {h:x}")


def cmd_eval(args):
    lib = Libm()
    vals = [float(v) for v in args.args]
    r = lib.call_bits(args.name, *(f2b(v) for v in vals))
    print(f"{args.name}{tuple(vals)} = {b2f(r)!r} ({r:#018x})")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    g = sub.add_parser("golden")
    g.add_argument("--out", default=str(ROOT / "crates" / "cw-math" / "tests" / "golden"))
    g.add_argument("--count", type=int, default=2000)
    g.add_argument("--seed", type=int, default=20130723)
    g.set_defaults(fn=cmd_golden)
    s = sub.add_parser("selftest")
    s.add_argument("--limit", type=int, default=20000)
    s.set_defaults(fn=cmd_selftest)
    hs = sub.add_parser("hash")
    hs.add_argument("name", choices=["asin", "acos", "exp", "pow", "pow_wide"])
    hs.add_argument("--count", type=int, default=200_000)
    hs.set_defaults(fn=cmd_hash)
    e = sub.add_parser("eval")
    e.add_argument("name", choices=sorted(ROUTINES))
    e.add_argument("args", nargs="+")
    e.set_defaults(fn=cmd_eval)
    a = ap.parse_args()
    a.fn(a)


if __name__ == "__main__":
    main()
