// Shader 05: cloud pixel shader (ps_3_0), Cube.exe RVA 0x2ff7c8 (VA 0x006ff7c8), 660 bytes.
// cube::CubeShader+0x10, created by 0x00447e10, bound with VS 00 by 0x00447d50 for the 7x7
// grid of drifting cloud models drawn at 0x004b1489.
// Source: analysis/shaders/05_ps_3_0_2ff7c8.asm.
// Constants: skyColor1 c0, skyColor2 c1, fogColor c2 (WorldPsConstants, world_common.wgsl).
//
//   out = lerp(vertexColor, sky_color(y) (rgba), 1 - (1 - h^5)^2),  h = sat(texcoord0.z)
// on all four channels, so the cloud alpha comes from the vertex colour and fades into the
// sky's alpha with distance. Deliberate deviations: none.

@fragment
fn fs_main(in: WorldPsIn) -> @location(0) vec4<f32> {
    let sky = sky_color(in.texcoord0.y);
    let h = saturate(in.texcoord0.z);
    var h4 = h * h;
    h4 = h4 * h4;
    var f = h4 * -h + 1.0;
    f = f * -f + 1.0;
    return f * (sky - in.color) + in.color;
}
