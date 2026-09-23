// Shared prelude of the world pixel shaders 01, 02, 03, 04 and 05 (Cube.exe
// cube::CubeShader, created by 0x00447e10). Prepended to each of them at load time
// (see gpu::wgsl), since WGSL has no #include.
//
// Constants (CTAB name -> register), one vec4 per register, the same layout in all five
// pixel shaders (04 and 02 declare fewer, the registers do not move):
//   skyColor1 c0, skyColor2 c1, fogColor c2, alpha c3.x (01 only), shininess c4.x (01 only).
// Set by the CubeShader uniform setters 0x00447fb0 (alpha), 0x00448fe0/0x00449040 (sky, fog).

struct WorldPsConstants {
    skyColor1: vec4<f32>,
    skyColor2: vec4<f32>,
    fogColor: vec4<f32>,
    alpha: vec4<f32>,
    shininess: vec4<f32>,
}

@group(0) @binding(1) var<uniform> ps: WorldPsConstants;

// The interpolants written by vertex shader 00 (vs00_world.wgsl).
struct WorldPsIn {
    @location(0) color: vec4<f32>,
    // (screen x, screen y + fogGradientTranslation, 3D distance^2 * fogScale^2,
    //  horizontal distance^2 * fogScale^2)
    @location(1) texcoord0: vec4<f32>,
    // 2 * view-space normal
    @location(2) texcoord1: vec3<f32>,
    // lightDirection
    @location(3) texcoord2: vec3<f32>,
    // cameraPosition - worldPos
    @location(4) texcoord3: vec3<f32>,
}

// 3t^2 - 2t^3, the compiler's `dp2add r, t^2, -t, 3t^2`.
fn smooth01(t: f32) -> f32 {
    let t2 = t * t;
    return t2 * -t + t2 * -t + t2 * 3.0;
}

// The sky colour behind a fragment at screen y (plus the gradient offset). All five pixel
// shaders contain this block; 02 is exactly this function with alpha 1.
//   grad    = lerp(skyColor1, skyColor2, smooth(sat(0.6 y)))
//   horizon = 0.2 skyColor1 + 0.8 fogColor
//   sky     = lerp(horizon, grad, smooth(sat(2 y + 1)))
// Returned as rgba: 05 blends the alpha too, the others take .rgb.
fn sky_color(y: f32) -> vec4<f32> {
    let g = smooth01(saturate(0.6 * y));
    let grad = g * (ps.skyColor2 - ps.skyColor1) + ps.skyColor1;
    let h = smooth01(saturate(y * 2.0 + 1.0));
    let horizon = ps.skyColor1 * 0.2 + 0.8 * ps.fogColor;
    return mix(horizon, grad, h);
}
