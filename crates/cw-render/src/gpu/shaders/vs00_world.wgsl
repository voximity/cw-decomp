// Shader 00: world vertex shader (vs_3_0), Cube.exe RVA 0x2fd928 (VA 0x006fd928), 4648 bytes.
// Created by cube::CubeShader::init 0x00447e10 (CubeShader+0x04), bound with pixel shaders
// 01 (0x00447d10), 02 (0x00447d90), 03 (0x00447dd0) and 05 (0x00447d50).
// Source: analysis/shaders/00_vs_3_0_2fd928.asm.
//
// Constants (CTAB name -> register), mirrored one vec4 per register so the CPU uploads the
// same 40 registers the original passes to SetVertexShaderConstantF:
//   pointLightPositions    c0..c9   (declared [16], the bytecode reads 10; .w = radius, on when > 0)
//   pointLightColors       c10..c19 (declared float3[16]; .xyz used)
//   worldViewProjMatrix    c20..c23
//   worldViewMatrix        c24..c26 (3 registers used)
//   worldMatrix            c27..c29 (3 registers used)
//   fogScale               c30.x
//   fogGradientTranslation c31.x
//   materialColor          c32
//   lightDirection         c33.xyz
//   lightFrontColor        c34
//   lightBackColor         c35
//   cameraPosition         c36.xyz
//   darknessColor          c37
//   ambientColor           c38
//   white                  c39.x
//
// Matrix convention: each mat column is one register, and `v * M` is the original's
// `dp4 v, cN` per register (component i = dot(v, register i)).
//
// Vertex fetch (the 8-byte world vertex, declaration made in 0x00447e10):
//   POSITION0 UBYTE4 @0: unnormalised 0..255, xyz = block position, w = face index.
//             Fetched as Uint8x4 and converted to float here (D3D9 did it in the fetch).
//   COLOR0 D3DCOLOR @4: bytes B,G,R,A in memory. Fetched as Unorm8x4 (-> B,G,R,A) and
//             swizzled .zyxw to R,G,B,A here, which is what D3DDECLTYPE_D3DCOLOR does.
//
// Outputs, as the original's o-registers:
//   color     (o1, COLOR0)     lit colour, alpha = vertex alpha (the light level)
//   texcoord0 (o2, TEXCOORD0)  (clip.x/w, clip.y/w + fogGradientTranslation,
//                               |camera - world|^2 * fogScale^2,           <- z: 3D distance
//                               |(camera - world).xy|^2 * fogScale^2)       <- w: horizontal
//             Note: analysis/shaders/README.md labels z/w the other way round; the bytecode
//             `mul o2.zw, r0.xywx, r1.x` writes r0.w (3D) to z and r0.x (horizontal) to w.
//   texcoord1 (o3, TEXCOORD1)  2 * normalize(worldViewMatrix * n)
//   texcoord2 (o4, TEXCOORD2)  lightDirection
//   texcoord3 (o5, TEXCOORD3)  cameraPosition - worldPos (unnormalised view vector)
//
// Darkness is `c = lit*tint + darknessColor*(1 - lit)` with `lit` the clamped light sum
// before tinting (README.md says `c += darknessColor*(1 - c)`; the bytecode computes
// `add r2.xyz, -r0, 1` before the multiply by the vertex and material colours).
//
// Deliberate deviations from the bytecode: none in the arithmetic. The 10 unrolled point
// lights are a loop.

struct WorldVsConstants {
    pointLightPositions: array<vec4<f32>, 10>,
    pointLightColors: array<vec4<f32>, 10>,
    worldViewProjMatrix: mat4x4<f32>,
    worldViewMatrix: mat3x4<f32>,
    worldMatrix: mat3x4<f32>,
    fogScale: vec4<f32>,
    fogGradientTranslation: vec4<f32>,
    materialColor: vec4<f32>,
    lightDirection: vec4<f32>,
    lightFrontColor: vec4<f32>,
    lightBackColor: vec4<f32>,
    cameraPosition: vec4<f32>,
    darknessColor: vec4<f32>,
    ambientColor: vec4<f32>,
    white: vec4<f32>,
}

@group(0) @binding(0) var<uniform> vs: WorldVsConstants;

struct WorldVertexIn {
    @location(0) position: vec4<u32>,
    @location(1) color_bgra: vec4<f32>,
}

struct WorldVsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) texcoord0: vec4<f32>,
    @location(2) texcoord1: vec3<f32>,
    @location(3) texcoord2: vec3<f32>,
    @location(4) texcoord3: vec3<f32>,
}

// Face index -> object-space normal: 0:+X 1:-X 2:+Y 3:-Y 4:+Z other:-Z
// (the sge/lrp chain of the bytecode).
fn face_normal(w: f32) -> vec3<f32> {
    var n = select(vec3<f32>(0.0, 0.0, -1.0), vec3<f32>(0.0, 0.0, 1.0), w == 4.0);
    n = select(n, vec3<f32>(0.0, -1.0, 0.0), w == 3.0);
    n = select(n, vec3<f32>(0.0, 1.0, 0.0), w == 2.0);
    n = select(n, vec3<f32>(-1.0, 0.0, 0.0), w == 1.0);
    n = select(n, vec3<f32>(1.0, 0.0, 0.0), w == 0.0);
    return n;
}

@vertex
fn vs_main(in: WorldVertexIn) -> WorldVsOut {
    var out: WorldVsOut;
    let pos = vec4<f32>(in.position);
    let vcolor = in.color_bgra.zyxw;

    // mad r0, v0.xyzx, (1,1,1,0), (0,0,0,1)
    let p = vec4<f32>(pos.xyz, 1.0);
    let clip = p * vs.worldViewProjMatrix;
    out.position = clip;
    let inv_w = 1.0 / clip.w;

    let world = p * vs.worldMatrix;
    let view_vec = vs.cameraPosition.xyz - world;
    let dist2 = dot(view_vec, view_vec);
    let horiz2 = view_vec.y * view_vec.y + view_vec.x * view_vec.x;
    let fog2 = vs.fogScale.x * vs.fogScale.x;
    out.texcoord0 = vec4<f32>(
        inv_w * clip.x,
        clip.y * inv_w + vs.fogGradientTranslation.x,
        dist2 * fog2,
        horiz2 * fog2,
    );
    out.texcoord3 = view_vec;

    let n = face_normal(pos.w);
    let wn = normalize(vec4<f32>(n, 0.0) * vs.worldMatrix);

    // Directional light: front * max(n.L, 0) + ambient + back * max(-n.L, 0),
    // scaled by materialColor.a * vertexColor.a.
    let l = vs.lightDirection.xyz;
    var light = vs.lightFrontColor.xyz * max(dot(wn, l), 0.0) + vs.ambientColor.xyz;
    light = vs.lightBackColor.xyz * max(dot(wn, -l), 0.0) + light;
    light = light * (vs.materialColor.w * vcolor.w);

    // Ten point lights (c0..c9 / c10..c19), not scaled by the vertex alpha.
    for (var i = 0; i < 10; i = i + 1) {
        let lp = vs.pointLightPositions[i];
        let on = select(0.0, 1.0, 0.0 < lp.w);
        let rel = world - lp.xyz;
        let d2 = dot(rel, rel);
        let dir = inverseSqrt(d2) * rel;
        let facing = min(max(dot(wn, dir) * -4.0, 0.3), 1.0);
        var fall = min(d2 * (1.0 / (lp.w * lp.w)), 1.0);
        fall = 1.0 - fall;
        fall = fall * fall;
        light = on * (vs.pointLightColors[i].xyz * (facing * fall)) + light;
    }

    // Combine: clamp to [0, 1.2], tint, darkness on (1 - light), flash to white.
    let lit = min(max(light, vec3<f32>(0.0)), vec3<f32>(1.2));
    let unlit = vec3<f32>(1.0) - lit;
    var c = lit * vcolor.xyz * vs.materialColor.xyz;
    c = vs.darknessColor.xyz * unlit + c;
    c = vs.white.x * (vec3<f32>(1.0) - c) + c;
    out.color = vec4<f32>(c, vcolor.w);

    let vn = normalize(vec4<f32>(n, 0.0) * vs.worldViewMatrix);
    out.texcoord1 = vn + vn;
    out.texcoord2 = vs.lightDirection.xyz;
    return out;
}
