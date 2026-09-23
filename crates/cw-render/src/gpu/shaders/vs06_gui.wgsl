// Shader 06: GUI vertex shader (vs_3_0), Cube.exe RVA 0x320558 (VA 0x00720558), 2252 bytes.
// plasma::D3D9Engine+0x194, created by D3D9Engine::init 0x0068a350, bound with PS 07 by
// bindWidgetShader 0x00688e70 (D3D9Engine slot 16) and D3D9Drawing::draw 0x0068ab70.
// Source: analysis/shaders/06_vs_3_0_320558.asm.
//
// Constants (CTAB name -> register), one vec4 per register:
//   Proj c0..c3, WorldView c4..c7, widgetBindMatrix c8..c11, inverseWidgetBindMatrix c12..c15,
//   MaskMatrix c16..c17, TextureMatrix c18..c19, NormalMatrix c20..c21,
//   widgetBindPos c22.xy, widgetBindSize c23.xy, deformedWidgetPos c24.xy,
//   deformedWidgetSize c25.xy, aaOffset c26.x, baseColor c27,
//   widgetDeformationEnabled b0 (a bool register; here the .x of a trailing vec4<u32>).
// Matrix convention: each mat column is one register, `v * M` = the original's dp4 per register.
//
// Vertex (48 bytes, declaration made in 0x0068a350): FLOAT2 POSITION0 @0, FLOAT4 COLOR0 @8,
// FLOAT2 TEXCOORD0 @24, TEXCOORD1 @32, TEXCOORD2 @40. D3D9 expands a FLOAT2 position to
// (x, y, 0, 1); done here explicitly.
//
// 1. Widget deformation (b0): p = pos * widgetBindMatrix; per axis
//    t = p < bindPos ? 0 : (p < bindPos + bindSize ? (p - bindPos) / bindSize : 1)
//    (a clamped ramp; README.md calls it a tent), q.xy = p.xy + deformedPos - bindPos
//    + t (deformedSize - bindSize), back through inverseWidgetBindMatrix.
// 2. Anti-aliasing fringe: TEXCOORD0/1 are two edge normals; when both are non-zero before
//    and after NormalMatrix, the view position moves by aaOffset / max(dot(n1, bisector), 0.7)
//    along the normalised bisector and z gets +0.5. The same xy offset is added to the
//    pre-WorldView position that feeds the mask coordinates.
// 3. position = Proj * view; color = baseColor * COLOR0; maskUV = MaskMatrix * pos;
//    textureUV = TextureMatrix * (TEXCOORD2, 0, 1).
//
// Deliberate deviation (TODO.md "Renderer quirks"): the -0.135 px offset the original
// subtracts from view x and y before Proj (`add r2.xyz, r1, c29.zzww`, c29.z = -0.135) is
// dropped. It was D3D9's hand-tuned half-pixel correction; wgpu samples pixel centres.

struct GuiVsConstants {
    Proj: mat4x4<f32>,
    WorldView: mat4x4<f32>,
    widgetBindMatrix: mat4x4<f32>,
    inverseWidgetBindMatrix: mat4x4<f32>,
    MaskMatrix: mat2x4<f32>,
    TextureMatrix: mat2x4<f32>,
    NormalMatrix: mat2x4<f32>,
    widgetBindPos: vec4<f32>,
    widgetBindSize: vec4<f32>,
    deformedWidgetPos: vec4<f32>,
    deformedWidgetSize: vec4<f32>,
    aaOffset: vec4<f32>,
    baseColor: vec4<f32>,
    widgetDeformationEnabled: vec4<u32>,
}

@group(0) @binding(0) var<uniform> vs: GuiVsConstants;

struct GuiVertexIn {
    @location(0) position: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) texcoord0: vec2<f32>,
    @location(3) texcoord1: vec2<f32>,
    @location(4) texcoord2: vec2<f32>,
}

struct GuiVsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    // o2 (TEXCOORD0): mask coordinates
    @location(1) mask_uv: vec2<f32>,
    // o3 (TEXCOORD1): texture coordinates
    @location(2) texture_uv: vec2<f32>,
}

fn step_lt(a: vec2<f32>, b: vec2<f32>) -> vec2<f32> {
    return select(vec2<f32>(0.0), vec2<f32>(1.0), a < b);
}

@vertex
fn vs_main(in: GuiVertexIn) -> GuiVsOut {
    var out: GuiVsOut;
    let v0 = vec4<f32>(in.position, 0.0, 1.0);

    var r0 = v0;
    if (vs.widgetDeformationEnabled.x != 0u) {
        let p = v0 * vs.widgetBindMatrix;
        let bind_pos = vs.widgetBindPos.xy;
        let bind_size = vs.widgetBindSize.xy;
        let below = step_lt(p.xy, bind_pos);
        let inside = step_lt(p.xy, bind_pos + bind_size);
        let ramp = (p.xy - bind_pos) * (vec2<f32>(1.0) / bind_size) - 1.0;
        let a = inside * ramp + 1.0;
        let t = below * -a + a;
        let shifted = p.xy + vs.deformedWidgetPos.xy - bind_pos;
        let grow = vs.deformedWidgetSize.xy - bind_size;
        let q = vec4<f32>(t * grow + shifted, p.z, p.w);
        r0 = q * vs.inverseWidgetBindMatrix;
    }

    let view = r0 * vs.WorldView;
    out.color = vs.baseColor * in.color;

    // Both edge normals present in the vertex?
    let raw_on = select(0.0, 1.0, 0.0 < dot(in.texcoord0, in.texcoord0))
        * select(0.0, 1.0, 0.0 < dot(in.texcoord1, in.texcoord1));
    let m1 = vec4<f32>(in.texcoord0, 0.0, 0.0) * vs.NormalMatrix;
    let m2 = vec4<f32>(in.texcoord1, 0.0, 0.0) * vs.NormalMatrix;
    let l1 = dot(m1, m1);
    let l2 = dot(m2, m2);
    let both = select(0.0, 1.0, 0.0 < l1) * select(0.0, 1.0, 0.0 < l2);
    // rsq, rcp, + 1e-5, rcp: division by (length + 1e-5)
    let n1 = m1 * (1.0 / (sqrt(l1) + 9.99999975e-6));
    let n2 = m2 * (1.0 / (sqrt(l2) + 9.99999975e-6));
    let b = n2 + n1;
    let bis = b * (1.0 / (sqrt(dot(b, b)) + 9.99999975e-6));
    let k = vs.aaOffset.x * (1.0 / max(dot(n1, bis), 0.699999988));
    let off = bis * k;
    let moved = vec3<f32>(off + view.xy, view.z);
    let delta = vec3<f32>(moved.xy - view.xy, (moved.z + 0.5) - view.z);

    r0 = vec4<f32>(raw_on * (both * vec3<f32>(off, 0.0)) + r0.xyz, r0.w);
    let view_pos = vec4<f32>(raw_on * (delta * both) + view.xyz, view.w);

    // The original subtracts (0.135, 0.135, 0) from view_pos here; dropped (see header).
    out.position = view_pos * vs.Proj;
    out.mask_uv = r0 * vs.MaskMatrix;
    out.texture_uv = vec4<f32>(in.texcoord2, 0.0, 1.0) * vs.TextureMatrix;
    return out;
}
