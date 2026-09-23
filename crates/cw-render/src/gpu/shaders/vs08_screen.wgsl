// Shader 08: screen-quad vertex shader (vs_3_0), Cube.exe RVA 0x3212c0 (VA 0x007212c0),
// 348 bytes. plasma::D3D9Engine+0x1a0, created by D3D9Engine::init 0x0068a350, used by every
// render-surface draw: drawCopy 0x0068c7c0 (+PS 09), drawBlurred 0x0068cad0 (+PS 13/14),
// drawScaled 0x0068cf10 (+PS 09/10/11/12). The quad comes from drawQuad 0x00689f30.
// Source: analysis/shaders/08_vs_3_0_3212c0.asm.
//
// Constants: Proj c0..c3 (column i = register i; `v * Proj` = dp4 per register).
// Vertex: the 48-byte GUI declaration; only POSITION0 (FLOAT2 -> (x, y, 0, 1)), COLOR0 and
// TEXCOORD2 are read (screen layout: shader locations 0, 1 and 4).
//
// Deliberate deviation (TODO.md "Renderer quirks"): the -0.135 px offset
// (`add r0, c4.xxyy, v0`) is dropped.

struct ScreenVsConstants {
    Proj: mat4x4<f32>,
}

@group(0) @binding(0) var<uniform> vs: ScreenVsConstants;

struct ScreenVertexIn {
    @location(0) position: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(4) texcoord2: vec2<f32>,
}

struct ScreenVsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    // o2 (TEXCOORD0)
    @location(1) uv: vec2<f32>,
}

@vertex
fn vs_main(in: ScreenVertexIn) -> ScreenVsOut {
    var out: ScreenVsOut;
    out.position = vec4<f32>(in.position, 0.0, 1.0) * vs.Proj;
    out.color = in.color;
    out.uv = in.texcoord2;
    return out;
}
