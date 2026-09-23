// Shader 09: render-surface blit (ps_3_0), Cube.exe RVA 0x321420 (VA 0x00721420), 272 bytes.
// plasma::D3D9Engine+0x1a4 ("copy"), created by 0x0068a350, bound with VS 08 by drawCopy
// 0x0068c7c0 (s0 MIN/MAG LINEAR) and by drawScaled 0x0068cf10 filter 1 (s0 MIN/MAG POINT).
// Source: analysis/shaders/09_ps_3_0_321420.asm.
// Constants: none. Sampler: textureSampler s0 (screen_common.wgsl).
//
//   out = unpremultiply(tex(uv)) * vertexColor
// Deliberate deviations: none.

@fragment
fn fs_main(in: ScreenPsIn) -> @location(0) vec4<f32> {
    let t = textureSample(textureTexture, textureSampler, in.uv);
    return unpremultiply(t) * in.color;
}
