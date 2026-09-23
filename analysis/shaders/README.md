# Cube.exe embedded Direct3D 9 shaders

Fifteen precompiled HLSL programs sit in `Cube.exe` `.rdata` (compiled by "Microsoft (R) HLSL
Shader Compiler 9.30.9200.16384", CTAB present in all of them). They were extracted with
`tools/extract_shaders.py` and disassembled with the system `d3dcompiler_47.dll`
(`D3DDisassemble`). The script also has a small fallback token disassembler, used only if
that DLL fails. Files:

- `NN_<kind>_<rva>.bin`: raw bytecode, from the version token to the `0x0000FFFF` END token.
- `NN_<kind>_<rva>.asm`: disassembly, including the CTAB parameter and register listing.
- `manifest.json`: file offset, RVA/VA, size, CTAB entries (name, register, class, type,
  rows, columns, elements), `dcl` inputs/outputs, samplers, `def` literals, instruction
  count, instruction slots, creator, owning object field, binding functions and pairing.
  The creator data comes from `creators.json`, which I filled in by hand from the Ghidra
  cross-references. The script merges it on every run.

Re-run: `.venv/Scripts/python.exe tools/extract_shaders.py`.

## Who creates and binds them

Two functions create all fifteen shaders. Each passes the bytecode address straight to
`IDirect3DDevice9::CreateVertexShader` (vtable +0x16c) or `CreatePixelShader` (+0x1a8):

| Creator | Class | Shaders | Vertex declaration made alongside |
|---|---|---|---|
| `0x00447e10` | `cube::CubeShader` (vtable `0x006ffa60`), called from `0x004c8720` | 00–05 | stride 8: `UBYTE4 POSITION0 @0`, `D3DCOLOR COLOR0 @4` |
| `0x0068a350` | `plasma::D3D9Engine` vtable slot 1 (`0x00722588`) | 06–14 | stride 48: `FLOAT2 POSITION0 @0`, `FLOAT4 COLOR0 @8`, `FLOAT2 TEXCOORD0 @24`, `FLOAT2 TEXCOORD1 @32`, `FLOAT2 TEXCOORD2 @40` |

Each creator also records the constant register numbers in its object (for example
`CubeShader+0x2c = 20` for `worldViewProjMatrix`). These numbers match the CTAB, so the C++
code uploads constants by fixed register numbers and never looks up the CTAB at run time.

Binding sites (the only `SetVertexShader`/`SetPixelShader` calls in the binary, found by
byte-scanning for `call [reg+0x170]` / `call [reg+0x1ac]`):

| Binder | VS + PS | Caller context |
|---|---|---|
| `0x00447d10` (bind default) | 00 + 01 | 20 call sites: terrain chunks, creatures, items, models (`0x004ac260` GameController::render, `0x005fc1b0`, `0x00605ae0`, …) |
| `0x00447d90` | 00 + 02 | `0x004adeb1`: sky background, `DrawPrimitiveUP(TRIANGLEFAN, 2)` with ZENABLE off |
| `0x00447d50` | 00 + 05 | `0x004b1489`: clouds, a 7×7 grid of time-drifting cloud models at height 170+50·noise |
| `0x00447dd0` | 00 + 03 | `0x004ba157`: water, the second index/vertex buffer of each zone. ZENABLE=1, ZWRITE=1, CULLMODE=NONE, then CULLMODE=CCW and back to 00+01 |
| none | 00 + 04 | `CubeShader+0x18` is created (`0x00447f8d`) and released, but never bound. Dead code. |
| `0x00688e70` (D3D9Engine slot 16), `0x0068ab70` (D3D9Drawing slot 2) | 06 + 07 | GUI widgets and tessellated vector shapes |
| `0x0068c7c0` (D3D9RenderSurface slot 5) | 08 + 09 | draws a render surface as a screen quad |
| `0x0068cf10` (D3D9RenderSurface slot 7) | 08 + 09/10/11/12 | filter 1 = plain blit, 2/3/4 = N×N box downsample |
| `0x0068cad0` (D3D9RenderSurface slot 6) | 08 + 14 or 13 | separable blur: flag true → 14 (horizontal), false → 13 (vertical) |

The render-surface quad is built by `0x00689f30`: a 4-vertex, 48-byte-stride VB with the
UVs in TEXCOORD2, and a 6-entry 32-bit index buffer `0,1,2,2,3,0`.

## Table

"Instr" is the count of executable instructions (no `dcl`/`def`). "Slots" is the compiler's
"approximately N instruction slots".

| # | Kind | Size | Instr / slots | Creator | Inputs read | Constants (CTAB name → register) | Samplers |
|---|---|---|---|---|---|---|---|
| 00 | vs_3_0 | 4648 | 236 / 240 | 0x00447e10 | POSITION0 (UBYTE4), COLOR0 | pointLightPositions c0 [16, 10 used], pointLightColors c10 [16, 10 used], worldViewProjMatrix c20, worldViewMatrix c24 (3 rows), worldMatrix c27 (3 rows), fogScale c30, fogGradientTranslation c31, materialColor c32, lightDirection c33, lightFrontColor c34, lightBackColor c35, cameraPosition c36, darknessColor c37, ambientColor c38, white c39 | – |
| 01 | ps_3_0 | 1100 | 39 / 50 | 0x00447e10 | COLOR0.rgb, TEXCOORD0, TEXCOORD1.xy | skyColor1 c0, skyColor2 c1, fogColor c2, alpha c3, shininess c4 | – |
| 02 | ps_3_0 | 556 | 17 / 19 | 0x00447e10 | TEXCOORD0.y | skyColor1 c0, skyColor2 c1, fogColor c2 | – |
| 03 | ps_3_0 | 1008 | 38 / 42 | 0x00447e10 | COLOR0.zw, TEXCOORD0.yz, TEXCOORD2, TEXCOORD3 | skyColor1 c0, skyColor2 c1, fogColor c2 | – |
| 04 | ps_3_0 | 520 | 16 / 16 | 0x00447e10 | COLOR0.rgb, TEXCOORD0.yz | skyColor1 c0, skyColor2 c1 | – |
| 05 | ps_3_0 | 660 | 22 / 24 | 0x00447e10 | COLOR0, TEXCOORD0.yz | skyColor1 c0, skyColor2 c1, fogColor c2 | – |
| 06 | vs_3_0 | 2252 | 92 / 94 | 0x0068a350 | POSITION0 (FLOAT2), COLOR0, TEXCOORD0, TEXCOORD1, TEXCOORD2 | widgetDeformationEnabled b0, Proj c0, WorldView c4, widgetBindMatrix c8, inverseWidgetBindMatrix c12, MaskMatrix c16 (2 rows), TextureMatrix c18 (2 rows), NormalMatrix c20 (2 rows), widgetBindPos c22, widgetBindSize c23, deformedWidgetPos c24, deformedWidgetSize c25, aaOffset c26, baseColor c27 | – |
| 07 | ps_3_0 | 1172 | 40 / 44 | 0x0068a350 | COLOR0, TEXCOORD0.xy, TEXCOORD1.xy | textureEnabled b0, filter c0 (int stored as float), textureOpacity c1, textureBrightness c2, textureContrast c3, textureSaturation c4 | maskSampler s0, textureSampler s1 |
| 08 | vs_3_0 | 348 | 7 / 7 | 0x0068a350 | POSITION0, COLOR0, TEXCOORD2 | Proj c0 | – |
| 09 | ps_3_0 | 272 | 5 / 5 | 0x0068a350 | COLOR0, TEXCOORD0.xy | – | textureSampler s0 |
| 10 | ps_3_0 | 584 | 20 / 20 | 0x0068a350 | COLOR0, TEXCOORD0.xy | textureScale c0 | textureSampler s0 |
| 11 | ps_3_0 | 800 | 34 / 34 | 0x0068a350 | COLOR0, TEXCOORD0.xy | textureScale c0 | textureSampler s0 |
| 12 | ps_3_0 | 1128 | 54 / 54 | 0x0068a350 | COLOR0, TEXCOORD0.xy | textureScale c0 | textureSampler s0 |
| 13 | ps_2_0 | 880 | 37 / 37 | 0x0068a350 | TEXCOORD0.xy (`t0`) | blurScale c0 | textureSampler s0 |
| 14 | ps_3_0 | 780 | 30 / 30 | 0x0068a350 | TEXCOORD0.xy | blurScale c0 | textureSampler s0 |

## What each program does

Conventions below: `smooth(t) = 3t² − 2t³` (the compiler's `dp2add` form), `sat` = clamp to
[0,1], `lerp(a,b,t) = a + t(b−a)`. The world is Z-up.

**00, world vertex shader (vs_3_0).** Takes `POSITION0` as UBYTE4 (unnormalised 0..255
integers): xyz is the block position inside the mesh and **w is a face index** mapped to the
normal 0:+X, 1:−X, 2:+Y, 3:−Y, 4:+Z, other (5):−Z. The position is transformed by
`worldViewProjMatrix`. `o2` (TEXCOORD0) carries
`(clip.x/w, clip.y/w + fogGradientTranslation, horizDist²·fogScale², dist3D²·fogScale²)`,
where distance is from `cameraPosition` to the world position (`worldMatrix`). Lighting per
vertex in world space:

- **Directional light:** `lightFrontColor·max(n·L,0) + ambientColor + lightBackColor·max(−n·L,0)`,
  scaled by `materialColor.a · vertexColor.a`. The vertex alpha works as a per-vertex light
  or occlusion factor. It is not transparency.
- **Point lights:** 10 are unrolled, although the arrays are declared [16]. Light i is on when
  `pointLightPositions[i].w > 0`, and `.w` is its radius. Its term is
  `pointLightColors[i] · (1 − min(d²/r², 1))² · clamp(−4·(n·dir), 0.3, 1)`.
- **Combine:** the light sum is clamped to [0, 1.2] and multiplied by `vertexColor.rgb ·
  materialColor.rgb`. Then `c += darknessColor·(1−c)`, then `lerp(c, 1, white)` (a flash to
  white). `o1.a = vertexColor.a` unchanged.

Extra outputs: `o3` = 2·normalize(worldViewMatrix·n) (view-space normal ×2), `o4` =
`lightDirection`, `o5` = `cameraPosition − worldPos` (unnormalised view vector).

**01, world pixel shader (the default for blocks, creatures, items).** Builds the sky
colour behind the fragment:

1. `grad = lerp(skyColor1, skyColor2, smooth(sat(0.6·y)))`, with y = TEXCOORD0.y
   (screen y + gradient offset).
2. `horizon = 0.2·skyColor1 + 0.8·fogColor`.
3. `sky = lerp(horizon, grad, smooth(sat(2y+1)))`.

The fragment colour is then:

1. Near fog: `lerp(vertexColor, fogColor, 0.75·(1 − (1 − sat(1.5·horiz² − 0.025))²))`.
2. Far fade to sky: `lerp(that, sky, smooth(sat(2.5·(dist3D² − 0.6)))²)`.
3. If `shininess > 0`, the rgb is multiplied by
   `1 + 0.3·sin(10·screenY + 5·screenX + nView.x + nView.y)`, an animated band shimmer that
   uses TEXCOORD1 = the view normal ×2.

Output alpha is the constant `alpha` (c3), set by `0x00447fb0`. The world uses this for
fading and ghosted creatures; see the COLORWRITEENABLE pre-pass below.

**02, sky background.** Only the sky-colour part of 01 (steps 1–3 above), with alpha 1.
It is drawn as a screen fan with ZENABLE off.

**03, water.** Specular: `pow(sat(normalize(normalize(lightDir + (0,0,2)) +
normalize(view)).z), 10) · vertexColor.a · skyColor1`, with the surface normal fixed as +Z.
Base colour: `(0, 0.6 − 0.5·sat(1 + screenY), 1)`, scaled by the lit blue channel
`vertexColor.b`. The result is lerped toward `fogColor` by `sat(horiz²)`, then toward the
01-style sky colour by `sat(2·(horiz² − 0.5))`. Output alpha is `0.8 + 1e-4·|view|²`, so
water gets more opaque with distance.

**04, unused.** A fog variant without `fogColor`. It blends `0.25/1.25·vertexColor` toward a
skyColor1/skyColor2 gradient with a cubic falloff on `sat(horiz²)`. It is never bound, so it
can be skipped in the port. It is kept here for completeness.

**05, clouds.** The sky colour of 01 in rgba (skyColor alpha is included), with
`vertexColor` as the base. Output is `lerp(vertexColor, sky, f)` on all four channels, with
`f = 1 − (1 − h⁵)²` with `h = sat(horiz²)`. The cloud alpha therefore comes
from the vertex colour and fades into the sky with distance.

**06, GUI vertex shader.**
- **Position:** POSITION0 is FLOAT2, so D3D expands it to (x,y,0,1).
- **Widget deformation:** if `widgetDeformationEnabled` (b0), the position is warped. It
  goes into widget bind space (`widgetBindMatrix`), is remapped from the bind rectangle
  (`widgetBindPos/Size`) to the deformed rectangle (`deformedWidgetPos/Size`) with a tent
  weighting, and comes back through `inverseWidgetBindMatrix`.
- **Anti-aliasing fringe:** TEXCOORD0 and TEXCOORD1 are two 2D edge normals. When both are
  non-zero, they go through `NormalMatrix`, the vertex is pushed out along their normalised
  bisector by `aaOffset / max(dot, 0.7)`, and z gets +0.5.
- **Outputs:** position = `Proj · (WorldView · pos − (0.135, 0.135, 0))` (a sub-pixel offset,
  see below). `o1 = baseColor · COLOR0`. `o2.xy = MaskMatrix · pos` (mask UV).
  `o3.xy = TextureMatrix · (TEXCOORD2.xy, 1)` (texture UV).

**07, GUI pixel shader.**
- **Texture path:** if `textureEnabled` (b0), it samples `textureSampler` (s1) at the texture
  UV. Saturation is applied as a lerp from grey (0.3, 0.59, 0.11) and skipped when it equals
  1. Brightness multiplies, skipped when 1. Contrast is `lerp(0.5, c, contrast)`, skipped when
  1. The result is clamped ≤1, and the output is `lerp(vertexColor, vertexColor·tex,
  textureOpacity)`.
- **No texture:** the output is `textureContrast · vertexColor`. This is how the compiler
  emitted it and should be kept.
- **Mask filter:** if `filter ≠ 0`, it samples `maskSampler` (s0) and **un-premultiplies**
  it (rgb/a when a>0). filter 1 → `lerp(color, (mask.rgb, 0), mask.a)`. filter 2 →
  `color·mask`. Any other value → `(color.rgb, color.a·mask.a)`. `0x00688e70` picks 0, 1,
  2 or 3 from the mask flags (`flags&2` → 2, else `((flags&4)|2)>>1`).

**08, screen-quad vertex shader.** `Proj · (pos − (0.135, 0.135, 0, 0))`. It passes
COLOR0 through and outputs TEXCOORD2 as TEXCOORD0.

**09, blit.** `unpremultiply(tex(uv)) · vertexColor`.

**10 / 11 / 12, N×N box downsample (N = 2, 3, 4).** The source is treated as a grid of
N-texel cells: `cell = floor(uv / textureScale)`. The shader averages the N² texels at
`(cell·N + 0.5 + k)·textureScale`, un-premultiplies, and multiplies by the vertex colour.
`textureScale` should be the source texel size. **See the original bug below.**

**13 / 14, 9-tap Gaussian blur.** Weights 0.05, 0.09, 0.12, 0.15, 0.18, 0.15, 0.12, 0.09,
0.05 at offsets −4..+4 × `blurScale`. 13 blurs along y and is compiled as ps_2_0. 14 blurs
along x (ps_3_0). No vertex colour and no un-premultiply. `blurScale = 0.25·radius / width`,
using the width for both directions. It is set by `0x0068cad0`, which also turns alpha
blending off and sets s0 to LINEAR/CLAMP for the pass.

## Shared code

- **Sky/fog gradient:** 01, 02, 03 and 05 (and the dead 04) all contain the same
  `skyColor1/skyColor2/fogColor` block: `smooth(sat(0.6y))`, `smooth(sat(2y+1))`,
  `0.2·sky1 + 0.8·fog`. In WGSL this is one function, `sky_color(y)`. 02 is exactly that
  function.
- **Box downsample:** 10, 11 and 12 are one template with N = 2, 3, 4 (weights 1/4, 1/9, 1/16).
- **Blur:** 13 and 14 are the same kernel with the axis swapped. The ps_2_0 vs ps_3_0
  difference has no semantic effect.
- **Un-premultiply:** 09–12 and the mask path of 07 all end with the same
  `rgb/a if a>0` + `· vertexColor`.
- **Screen quad:** 08 serves every render-surface pass. 06 and 08 share `Proj c0` and the
  −0.135 xy offset.

## What the bytecode leaves to the fixed-function pipeline

- **No alpha test anywhere.** None of the shaders uses `texkill`/`clip`. Any cut-out has
  to come from `D3DRS_ALPHATESTENABLE` (not seen set in the static scan; confirm with the
  capture) or from blending.
- **Fog is entirely in the shaders.** No shader writes `oFog`, and no FOGENABLE /
  FOGTABLEMODE writes were found. The port must not add hardware fog.
- **Blending:**
  - GUI: `0x00688b60` sets `SRCBLEND=SRCALPHA, DESTBLEND=INVSRCALPHA`, with separate alpha
    `ONE, INVSRCALPHA`. `0x006897c0`/`0x00689950` temporarily switch to
    `BLENDOP=SUBTRACT, ONE/ONE`.
  - World: `GameController::render` enables `ALPHABLENDENABLE` with `SRCALPHA/INVSRCALPHA`
    (RS 19=5, 20=6 at `0x004aedd5..`). 01's constant `alpha`, 03's distance alpha and 05's
    vertex alpha depend on this.
- **Ghosted creatures:** `COLORWRITEENABLE=0` first (`0x004ba890`, a depth-only draw
  through `0x004128f0`), then `COLORWRITEENABLE=0xF` (`0x004ba90f`) and the same model is
  drawn again with `alpha = 1 − 0.75·creature[+0x1190]`. This is a depth pre-pass so that
  only the nearest surface of a translucent model shows.
- **Depth and culling:** ZENABLE is off for the sky (02) and for GUI and screen quads.
  ZWRITE is on for water. CULLMODE is NONE (1) for water and CCW (2) otherwise. The static
  scan shows ZFUNC set to 2 (`D3DCMP_LESS`) in `0x004758c0`/`0x00476660`/`0x004d50a0` and to
  5 (`D3DCMP_GREATER`) in `0x0068ab70` (purpose not checked). All other states should
  come from the render capture.
- **Samplers:** LINEAR for blits/blur (`0x0068c7c0`, `0x0068cad0`), POINT for the
  downsamples (`0x0068cf10`), CLAMP for blur, BORDER (4) on s1 for GUI (`0x00688b60`).
- **Vertex fetch conversions:** UBYTE4 → unnormalised float, where a wgpu `Uint8x4` must
  be cast to float in the shader. D3DCOLOR → BGRA-swizzled normalised float: use
  `Unorm8x4` and swizzle `.bgra`, or store the colour RGBA. FLOAT2 POSITION → (x,y,0,1), and
  FLOAT2 TEXCOORD → (x,y,0,1).
- **Half-pixel offset:** D3D9's pixel-centre offset is hand-tuned as −0.135 px in 06 and
  08, not the usual −0.5. wgpu has no half-pixel offset. The port should probably drop it
  (Tier C), but it shifts GUI rasterisation by ~0.135 px against the capture.
- **Premultiplied textures:** GUI textures and render surfaces are treated as premultiplied
  and un-premultiplied in the shader before blending with SRCALPHA/INVSRCALPHA.
- **Point lights:** vs 00 only reads 10 of the 16 array entries. The upload in the game uses
  a light count capped at 16 (`0x004c14d0`), so lights 10–15 are silently dropped, or they
  overlap `pointLightColors` at c10 if the C++ uploads 16 positions. Check the upload order.

## Original bugs to preserve or fix consciously

- `0x0068cf10` (RenderSurface downsample, filters 2–4) computes `(1/width, 1/height)` for
  `textureScale` but passes it to **`GetPixelShaderConstantF`** (device vtable +0x1b8,
  confirmed at `0x0068d1bf`) instead of `SetPixelShaderConstantF` (+0x1b4). The shaders
  therefore read whatever PS c0 held before: the blur's `blurScale`, the GUI `filter`, or
  0 → `rcp` = inf → NaN. Either the filter 2–4 path is never taken in practice, or its output
  is garbage. Check this with the capture before porting 10–12.

## Corrections from the WGSL rewrite (2026-09-23)

The WGSL in `crates/cw-render/src/gpu/shaders/` follows the bytecode where this README's
prose differs: VS 00 writes `o2.z` = the 3D distance squared and `o2.w` = the horizontal
distance squared (swapped above), so PS 01's near fog uses the 3D distance and its far fade
the horizontal one, while 03 and 05 use the 3D distance; the darkness term is
`lit * tint + darknessColor * (1 - lit)` with `lit` the clamped light sum before tinting; the
01 shimmer is `cos`, not `sin`; PS 07's filter mode 1 keeps `c.a`; VS 06's deformation is a
clamped 0..1 ramp, not a tent; only the directional and ambient light is scaled by
`materialColor.a * vertexAlpha`, the point lights are not. Treat the `.asm` files as the
reference.
From the render-pass port: D3DCULLMODE 2 is D3DCULL_CW, not CCW; the water pass and the fixup after it set ZWRITE 1.
