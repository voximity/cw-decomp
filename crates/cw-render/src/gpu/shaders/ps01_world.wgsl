// Shader 01: world pixel shader (ps_3_0), Cube.exe RVA 0x2feb50 (VA 0x006feb50), 1100 bytes.
// cube::CubeShader+0x08, created by 0x00447e10, bound with VS 00 by 0x00447d10 (the default:
// terrain chunks, creatures, items, models). Source: analysis/shaders/01_ps_3_0_2feb50.asm.
// Constants: skyColor1 c0, skyColor2 c1, fogColor c2, alpha c3, shininess c4
// (WorldPsConstants in world_common.wgsl, prepended).
//
// 1. near fog: lerp(vertexColor, fogColor, 0.75 (1 - (1 - sat(1.5 d3 - 0.025))^2)),
//    d3 = texcoord0.z (3D distance^2 * fogScale^2)
// 2. far fade: lerp(that, sky_color(y), smooth(sat(2.5 (dh - 0.6)))^2),
//    dh = texcoord0.w (horizontal distance^2 * fogScale^2)
// 3. if shininess > 0: rgb *= 1 + 0.3 cos(10 y + 5 x + nView.x + nView.y)
// Output alpha is the constant `alpha` (fading, ghosted creatures).
//
// Deliberate deviations: none. Notes against README.md: the shimmer is `sincos` .x, i.e. a
// cosine (the README says sin), and the fog terms read z = 3D and w = horizontal distance.
// The bytecode's frc/mad range reduction before sincos is dropped: WGSL cos takes any angle.

@fragment
fn fs_main(in: WorldPsIn) -> @location(0) vec4<f32> {
    let sky = sky_color(in.texcoord0.y).xyz;

    var far = smooth01(saturate((in.texcoord0.w - 0.6) * 2.5));
    far = far * far;

    var near = 1.0 - saturate(in.texcoord0.z * 1.5 - 0.025);
    near = (near * -near + 1.0) * 0.75;

    let fogged = mix(in.color.xyz, ps.fogColor.xyz, near);
    let c = mix(fogged, sky, far);

    let angle = in.texcoord0.y * 10.0 + in.texcoord0.x * 5.0 + in.texcoord1.y + in.texcoord1.x;
    let shimmer = c * (cos(angle) * 0.3 + 1.0);
    // cmp oC0.xyz, -shininess, r3, r0: -shininess >= 0 keeps the plain colour.
    let rgb = select(shimmer, c, -ps.shininess.x >= 0.0);
    return vec4<f32>(rgb, ps.alpha.x);
}
