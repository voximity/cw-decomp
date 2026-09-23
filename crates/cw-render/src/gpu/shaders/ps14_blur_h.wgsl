// Shader 14: horizontal 9-tap Gaussian blur (ps_3_0), Cube.exe RVA 0x322270 (VA 0x00722270),
// 780 bytes. plasma::D3D9Engine+0x1b4, created by 0x0068a350, bound with VS 08 by drawBlurred
// 0x0068cad0 when its flag is true. Source: analysis/shaders/14_ps_3_0_322270.asm.
// Constants: blurScale c0.x (BlurPsConstants) = 0.25 * radius / surface width.
// Sampler: textureSampler s0, LINEAR/CLAMP for the pass, alpha blending off.
//
//   out = sum_k w_k tex(uv + (k * blurScale, 0)), k = -4..4
// Deliberate deviations: none.

@group(0) @binding(1) var<uniform> ps: BlurPsConstants;

@fragment
fn fs_main(in: ScreenPsIn) -> @location(0) vec4<f32> {
    return blur9(in.uv, vec2<f32>(ps.blurScale.x, 0.0));
}
