// Shader 10: 2x2 box downsample (ps_3_0), Cube.exe RVA 0x321530 (VA 0x00721530), 584 bytes.
// plasma::D3D9Engine+0x1a8, created by 0x0068a350, bound with VS 08 by drawScaled 0x0068cf10
// filter 2 (s0 MIN/MAG POINT). Source: analysis/shaders/10_ps_3_0_321530.asm.
// Constants: textureScale c0.xy (DownsamplePsConstants). Sampler: textureSampler s0.
//
//   out = unpremultiply(average of the 2x2 source cell) * vertexColor
// Deliberate deviation: the cell addressing and the textureScale upload are fixed, see
// box_downsample in screen_common.wgsl.

@group(0) @binding(1) var<uniform> ps: DownsamplePsConstants;

@fragment
fn fs_main(in: ScreenPsIn) -> @location(0) vec4<f32> {
    return box_downsample(in.uv, ps.textureScale.xy, 2) * in.color;
}
