// Shader 03: water pixel shader (ps_3_0), Cube.exe RVA 0x2ff1d0 (VA 0x006ff1d0), 1008 bytes.
// cube::CubeShader+0x14, created by 0x00447e10, bound with VS 00 by 0x00447dd0 for the
// second index/vertex buffer of each zone (0x004ba157: ZENABLE 1, ZWRITE 1, CULLMODE NONE).
// Source: analysis/shaders/03_ps_3_0_2ff1d0.asm.
// Constants: skyColor1 c0, skyColor2 c1, fogColor c2 (WorldPsConstants, world_common.wgsl).
// Reads COLOR0.zw, TEXCOORD0.yz, TEXCOORD2 (lightDirection) and TEXCOORD3 (view vector).
//
//   spec  = pow(sat(normalize(normalize(L + (0,0,2)) + normalize(V)).z), 10) * color.a * skyColor1
//   base  = (0, 0.6 - 0.5 sat(1 + y), 1) * color.b + spec
//   rgb   = lerp(lerp(base, fogColor, sat(d3)), sky_color(y), sat(2 (d3 - 0.5)))
//   alpha = 0.8 + 1e-4 |V|^2
// with d3 = texcoord0.z (3D distance^2 * fogScale^2).
//
// Deliberate deviation: the alpha is saturated here. D3D9 clamps oC0 to [0,1] for the
// A8R8G8B8/X8R8G8B8 target before blending; unorm attachments in wgpu backends do the same,
// the explicit saturate only makes that independent of the backend.

@fragment
fn fs_main(in: WorldPsIn) -> @location(0) vec4<f32> {
    let y = in.texcoord0.y;
    let d3 = in.texcoord0.z;
    let sky = sky_color(y).xyz;

    let h = in.texcoord2 + vec3<f32>(0.0, 0.0, 2.0);
    let hn = h * inverseSqrt(dot(h, h));
    let v2 = dot(in.texcoord3, in.texcoord3);
    let alpha = v2 * 9.99999975e-5 + 0.8;
    let vn = inverseSqrt(v2) * in.texcoord3;
    let half_vec = hn + vn;
    let nz = saturate(inverseSqrt(dot(half_vec, half_vec)) * half_vec.z);
    let spec = (pow(nz, 10.0) * in.color.w) * ps.skyColor1.xyz;

    let deep = saturate(1.0 + y);
    let base = deep * vec3<f32>(0.0, -0.5, 0.0) + vec3<f32>(0.0, 0.6, 1.0);
    let lit = in.color.z * base + spec;

    let fogged = mix(lit, ps.fogColor.xyz, saturate(d3));
    let to_sky = saturate((d3 - 0.5) + (d3 - 0.5));
    let rgb = to_sky * (sky - fogged) + fogged;
    return vec4<f32>(rgb, saturate(alpha));
}
