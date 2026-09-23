// Shader 13: vertical 9-tap Gaussian blur (ps_2_0), Cube.exe RVA 0x321f00 (VA 0x00721f00),
// 880 bytes. plasma::D3D9Engine+0x1b8, created by 0x0068a350, bound with VS 08 by drawBlurred
// 0x0068cad0 when its flag is false. Source: analysis/shaders/13_ps_2_0_321f00.asm.
// Constants: blurScale c0.x (BlurPsConstants) = 0.25 * radius / surface width (the width is
// used for both directions). Sampler: textureSampler s0, LINEAR/CLAMP for the pass, with alpha
// blending off.
//
//   out = sum_k w_k tex(uv + (0, k * blurScale)), k = -4..4
// Deliberate deviations: none (ps_2_0 vs ps_3_0 has no semantic effect; it reads t0, which
// VS 08 writes as TEXCOORD0).

@group(0) @binding(1) var<uniform> ps: BlurPsConstants;

@fragment
fn fs_main(in: ScreenPsIn) -> @location(0) vec4<f32> {
    return blur9(in.uv, vec2<f32>(0.0, ps.blurScale.x));
}
