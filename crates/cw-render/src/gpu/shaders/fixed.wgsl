// The D3D9 fixed-function pipeline as Cube.exe uses it (not one of the fifteen embedded
// shaders): `SetVertexShader(NULL)`, `SetPixelShader(NULL)`, `D3DRS_LIGHTING = FALSE`, fog
// off (FOGENABLE is never set), FVF 0x42 (XYZ|DIFFUSE) or 0x142 (XYZ|DIFFUSE|TEX1). Used by
// the stars (0x004ae641), the sun (0x004aea83), the ground decals and blob shadows
// (0x004b8bc7) and the ribbons (0x004b9993) of GameController::render 0x004ac260.
//
// Transform: position * WORLD * VIEW * PROJECTION (SetTransform), uploaded as four registers
// (the columns of the D3D row-vector matrix), `v * M` as in vertex shader 00.
//
// Texture stage 0 keeps the device defaults (Cube.exe never calls SetTextureStageState):
// COLOROP MODULATE(TEXTURE, CURRENT = diffuse), ALPHAOP SELECTARG1(TEXTURE). With no texture
// bound the stage passes the diffuse colour and alpha through (flags.x = 0 here).
//
// Vertex fetch: FLOAT3 position, D3DCOLOR diffuse (bytes B, G, R, A in memory, fetched as
// Unorm8x4 and swizzled .zyxw), FLOAT2 uv (ignored for FVF 0x42).

struct FixedConstants {
    wvp: mat4x4<f32>,
    // x: 1 when stage 0 has a texture (FVF 0x142 with SetTexture(0, tex)).
    flags: vec4<f32>,
}

@group(0) @binding(0) var<uniform> c: FixedConstants;
@group(1) @binding(0) var stage0_texture: texture_2d<f32>;
@group(1) @binding(1) var stage0_sampler: sampler;

struct FixedIn {
    @location(0) position: vec3<f32>,
    @location(1) diffuse_bgra: vec4<f32>,
    @location(2) uv: vec2<f32>,
}

struct FixedOut {
    @builtin(position) position: vec4<f32>,
    @location(0) diffuse: vec4<f32>,
    @location(1) uv: vec2<f32>,
}

@vertex
fn vs_main(v: FixedIn) -> FixedOut {
    var o: FixedOut;
    o.position = vec4<f32>(v.position, 1.0) * c.wvp;
    o.diffuse = v.diffuse_bgra.zyxw;
    o.uv = v.uv;
    return o;
}

@fragment
fn fs_main(i: FixedOut) -> @location(0) vec4<f32> {
    let t = textureSample(stage0_texture, stage0_sampler, i.uv);
    if (c.flags.x > 0.5) {
        return vec4<f32>(t.rgb * i.diffuse.rgb, t.a);
    }
    return i.diffuse;
}
