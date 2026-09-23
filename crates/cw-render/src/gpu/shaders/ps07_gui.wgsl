// Shader 07: GUI pixel shader (ps_3_0), Cube.exe RVA 0x320e28 (VA 0x00720e28), 1172 bytes.
// plasma::D3D9Engine+0x198, created by D3D9Engine::init 0x0068a350, bound with VS 06 by
// 0x00688e70 and 0x0068ab70. Source: analysis/shaders/07_ps_3_0_320e28.asm.
//
// Constants (CTAB name -> register), one vec4 per register:
//   filter c0.x (an int stored as float; 0x00688e70 picks 0..3 from the mask flags),
//   textureOpacity c1.x, textureBrightness c2.x, textureContrast c3.x,
//   textureSaturation c4.x, textureEnabled b0 (here the .x of a trailing vec4<u32>).
// Samplers: maskSampler s0 -> group(1) binding(0/1), textureSampler s1 -> binding(2/3).
// beginFrame 0x00688b60 sets s1 ADDRESSU/V = BORDER (border colour default 0); the
// per-texture state comes from D3D9Texture::bind 0x0068bc90 (see gpu::textures).
//
// Texture path (b0): saturation lerp from grey (0.3, 0.59, 0.11), skipped when == 1;
// brightness multiply, skipped when == 1; contrast lerp(0.5, c, contrast), skipped when == 1;
// min(rgb, 1); out = lerp(color, color * tex, textureOpacity).
// No texture: out = textureContrast * color (as the compiler emitted it).
// Mask (filter != 0): the mask is un-premultiplied (rgb / a when a > 0), then
//   filter 1: (lerp(c.rgb, mask.rgb, mask.a), c.a)   (README.md says alpha 0 is lerped
//             in; the bytecode sets r3.w = 0 before `mad r3, r2.w, r3, r0`, so alpha
//             stays c.a)
//   filter 2: c * mask
//   other:    (c.rgb, c.a * mask.a)
//
// Deliberate deviations: none. `filter` is a reserved word in WGSL, so the member is
// spelled `filter_`; every other name is the CTAB's.

struct GuiPsConstants {
    filter_: vec4<f32>,
    textureOpacity: vec4<f32>,
    textureBrightness: vec4<f32>,
    textureContrast: vec4<f32>,
    textureSaturation: vec4<f32>,
    textureEnabled: vec4<u32>,
}

@group(0) @binding(1) var<uniform> ps: GuiPsConstants;
@group(1) @binding(0) var maskTexture: texture_2d<f32>;
@group(1) @binding(1) var maskSampler: sampler;
@group(1) @binding(2) var textureTexture: texture_2d<f32>;
@group(1) @binding(3) var textureSampler: sampler;

struct GuiPsIn {
    @location(0) color: vec4<f32>,
    @location(1) mask_uv: vec2<f32>,
    @location(2) texture_uv: vec2<f32>,
}

@fragment
fn fs_main(in: GuiPsIn) -> @location(0) vec4<f32> {
    let color = in.color;
    var c: vec4<f32>;
    if (ps.textureEnabled.x != 0u) {
        let tex = textureSample(textureTexture, textureSampler, in.texture_uv);
        let sat = ps.textureSaturation.x;
        let grey = dot(vec3<f32>(0.300000012, 0.589999974, 0.109999999), tex.xyz);
        let desat = (1.0 - sat) * (vec3<f32>(grey) - tex.xyz) + tex.xyz;
        var rgb = select(desat, tex.xyz, -abs(sat - 1.0) >= 0.0);
        let bri = ps.textureBrightness.x;
        rgb = select(rgb * bri, rgb, -abs(bri - 1.0) >= 0.0);
        let con = ps.textureContrast.x;
        let contrasted = (1.0 - con) * (vec3<f32>(0.5) - rgb) + rgb;
        rgb = select(contrasted, rgb, -abs(con - 1.0) >= 0.0);
        let t = vec4<f32>(min(rgb, vec3<f32>(1.0)), tex.w);
        c = ps.textureOpacity.x * (color * t - color) + color;
    } else {
        c = ps.textureContrast.x * color;
    }

    if (ps.filter_.x == 0.0) {
        return c;
    }

    let m = textureSample(maskTexture, maskSampler, in.mask_uv);
    let mrgb = select((1.0 / m.w) * m.xyz, m.xyz, -m.w >= 0.0);
    let f1 = vec4<f32>(m.w * (mrgb - c.xyz) + c.xyz, c.w);
    let prod = c * vec4<f32>(mrgb, m.w);
    let other = select(vec4<f32>(c.xyz, prod.w), prod, -abs(ps.filter_.x - 2.0) >= 0.0);
    return select(other, f1, -abs(ps.filter_.x - 1.0) >= 0.0);
}
