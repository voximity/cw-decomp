// Shader 02: sky background pixel shader (ps_3_0), Cube.exe RVA 0x2fefa0 (VA 0x006fefa0),
// 556 bytes. cube::CubeShader+0x0c, created by 0x00447e10, bound with VS 00 by 0x00447d90
// for the screen fan drawn at 0x004adeb1 with ZENABLE off.
// Source: analysis/shaders/02_ps_3_0_2fefa0.asm.
// Constants: skyColor1 c0, skyColor2 c1, fogColor c2 (WorldPsConstants, world_common.wgsl).
//
// Exactly sky_color(y) with alpha 1. Deliberate deviations: none.

@fragment
fn fs_main(in: WorldPsIn) -> @location(0) vec4<f32> {
    return vec4<f32>(sky_color(in.texcoord0.y).xyz, 1.0);
}
