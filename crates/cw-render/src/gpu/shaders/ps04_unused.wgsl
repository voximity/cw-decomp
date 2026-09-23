// Shader 04: unused world pixel shader (ps_3_0), Cube.exe RVA 0x2ff5c0 (VA 0x006ff5c0),
// 520 bytes. cube::CubeShader+0x18, created at 0x00447f8d and released, never bound (dead
// code). Translated for completeness only. Source: analysis/shaders/04_ps_3_0_2ff5c0.asm.
// Constants: skyColor1 c0, skyColor2 c1 (WorldPsConstants, world_common.wgsl; fogColor unused).
// Reads COLOR0.rgb and TEXCOORD0.yz.
//
// Transcription of the bytecode (the README's summary is looser):
//   a    = sat(1 + y)
//   g    = lerp(skyColor1, skyColor2, sat(y))
//   far  = lerp((0.8, 1, 1), g, a)
//   base = (0.25 r, 0.25 g, b) of the vertex colour
//   near = lerp(base, skyColor2, min(1.25 h, 1)),          h = sat(texcoord0.z)
//   rgb  = h == 0 ? near : lerp(near, far, h^3);  alpha 1
// Deliberate deviations: none.

@fragment
fn fs_main(in: WorldPsIn) -> @location(0) vec4<f32> {
    let a = saturate(1.0 + in.texcoord0.y);
    let h = saturate(in.texcoord0.z);
    let t = saturate(in.texcoord0.y);
    let g = mix(ps.skyColor1.xyz, ps.skyColor2.xyz, t) + vec3<f32>(-0.8, -1.0, -1.0);
    let far = a * g + vec3<f32>(0.8, 1.0, 1.0);
    let base = vec3<f32>(0.25, 0.25, 1.0) * in.color.xyz;
    let to_sky = ps.skyColor2.xyz - in.color.xyz * vec3<f32>(0.25, 0.25, 1.0);
    let near = min(h * 1.25, 1.0) * to_sky + base;
    let h3 = h * (h * h);
    let blended = mix(near, far, h3);
    let rgb = select(blended, near, -h >= 0.0);
    return vec4<f32>(rgb, 1.0);
}
