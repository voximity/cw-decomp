// Shared prelude of the render-surface pixel shaders 09..14 (plasma::D3D9Engine, created by
// 0x0068a350), prepended at load time (see gpu::wgsl). Sampler s0 is `textureSampler`.

@group(1) @binding(0) var textureTexture: texture_2d<f32>;
@group(1) @binding(1) var textureSampler: sampler;

// The interpolants written by vertex shader 08 (vs08_screen.wgsl).
struct ScreenPsIn {
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
}

// rgb / a when a > 0 (`rcp; mul; cmp -a`), as 07, 09, 10, 11 and 12 all do.
fn unpremultiply(t: vec4<f32>) -> vec4<f32> {
    let rgb = select(t.xyz * (1.0 / t.w), t.xyz, -t.w >= 0.0);
    return vec4<f32>(rgb, t.w);
}

// Constants of the downsample shaders 10, 11 and 12: textureScale c0.xy.
struct DownsamplePsConstants {
    textureScale: vec4<f32>,
}

// Constants of the blur shaders 13 and 14: blurScale c0.x.
struct BlurPsConstants {
    blurScale: vec4<f32>,
}

// The N x N box downsample template of shaders 10 (N = 2), 11 (N = 3) and 12 (N = 4):
// average the N^2 texels of the source cell under this fragment, un-premultiply, tint.
//
// `texel` is the SOURCE texel size (1/width, 1/height), which is what drawScaled 0x0068cf10
// computes. The original shader takes `cell = floor(uv / textureScale)` and samples
// `(cell * N + 0.5 + k) * textureScale`, which only addresses the right texels if uv is
// measured in destination cells; and 0x0068cf10 hands the value to GetPixelShaderConstantF
// (device +0x1b8) instead of SetPixelShaderConstantF (+0x1b4), so the shader read a stale c0.
// Deliberate deviation (TODO.md "Renderer quirks"): cell = floor(uv / (N * texel)), so a
// quad whose uv spans the source maps each destination pixel to its N x N source block.
fn box_downsample(uv: vec2<f32>, texel: vec2<f32>, n: i32) -> vec4<f32> {
    let nf = f32(n);
    let cell = floor(uv / (texel * nf));
    let base = cell * nf + 0.5;
    var sum = vec4<f32>(0.0);
    for (var j = 0; j < n; j = j + 1) {
        for (var i = 0; i < n; i = i + 1) {
            let at = (base + vec2<f32>(f32(i), f32(j))) * texel;
            sum = sum + textureSampleLevel(textureTexture, textureSampler, at, 0.0);
        }
    }
    return unpremultiply(sum * (1.0 / (nf * nf)));
}

// The 9-tap kernel of shaders 13 (vertical) and 14 (horizontal): weights 0.05, 0.09, 0.12,
// 0.15, 0.18, 0.15, 0.12, 0.09, 0.05 at offsets -4..+4 times `step`. No vertex colour and no
// un-premultiply.
fn blur9(uv: vec2<f32>, step: vec2<f32>) -> vec4<f32> {
    var w = array<f32, 9>(
        0.0500000007, 0.0900000036, 0.119999997, 0.150000006, 0.180000007,
        0.150000006, 0.119999997, 0.0900000036, 0.0500000007,
    );
    var sum = vec4<f32>(0.0);
    for (var k = 0; k < 9; k = k + 1) {
        let at = uv + f32(k - 4) * step;
        sum = sum + textureSampleLevel(textureTexture, textureSampler, at, 0.0) * w[k];
    }
    return sum;
}
