//! The GPU layer: the wgpu reimplementation of Cube.exe's `plasma::D3D9Engine` and
//! `cube::CubeShader` (Tier C).
//!
//! - [`device`]: instance, adapter, device, queue and the window surface
//!   (replaces the IDirect3D9/IDirect3DDevice9 created in the startup code).
//! - [`wgsl`]: the fifteen embedded D3D9 shaders rewritten in WGSL (`shaders/*.wgsl`,
//!   one file per original program; see `analysis/shaders/README.md`).
//! - [`pipelines`]: the vertex layouts (8-byte world, 48-byte GUI, screen quad), the
//!   constant-register structs, bind group layouts and one `wgpu::RenderPipeline` per
//!   (vertex shader, pixel shader, blend/depth/cull state) combination the original uses.
//! - [`targets`]: `plasma::D3D9RenderSurface` as an offscreen colour + depth texture, with
//!   the copy, blur and downsample draws.
//! - [`exec`]: the executor walking a `FrameCommands` (resources, uniform ring, pipeline
//!   cache, fixed-function replacement, GUI render-surface chain).
//! - [`textures`]: `plasma::D3D9Texture` creation (RGBA8, PNG through `image`) and the
//!   per-texture sampler state of `D3D9Texture::bind` 0x0068bc90.
//!
//! Colour handling: D3D9 blended and presented in gamma space with no sRGB conversion
//! anywhere (A8R8G8B8 back buffer and textures, no D3DRS_SRGBWRITEENABLE). To keep the
//! visible result, every colour target and texture here is a *non-sRGB* unorm format and
//! shaders write the values the original wrote; see [`device::GpuContext::color_format`].

pub mod device;
pub mod exec;
/// Port-only: the pass drawn over the finished frame (the client's debug overlay).
pub mod overlay;
pub mod pipelines;
pub mod targets;
pub mod textures;

/// The WGSL rewrites of the fifteen shaders embedded in Cube.exe `.rdata`.
///
/// Shaders that share code with others get a prelude prepended (WGSL has no `#include`):
/// `world_common.wgsl` (the `skyColor1/skyColor2/fogColor` block and the pixel constants)
/// for 01..05, `screen_common.wgsl` (un-premultiply, box downsample, 9-tap blur) for 09..14.
pub mod wgsl {
    /// Programmable stage of an original shader.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Stage {
        /// `vs_3_0`
        Vertex,
        /// `ps_3_0` / `ps_2_0`
        Fragment,
    }

    /// One original shader and its WGSL rewrite.
    #[derive(Clone, Copy, Debug)]
    pub struct ShaderInfo {
        /// Index used by `analysis/shaders/` (00..14).
        pub index: u8,
        /// Short name.
        pub name: &'static str,
        /// RVA of the bytecode in Cube.exe.
        pub rva: u32,
        /// Stage.
        pub stage: Stage,
        /// Complete WGSL module source (prelude included). Entry point `vs_main` or `fs_main`.
        pub source: &'static str,
    }

    macro_rules! world_ps {
        ($file:literal) => {
            concat!(
                include_str!("shaders/world_common.wgsl"),
                "\n",
                include_str!($file)
            )
        };
    }
    macro_rules! screen_ps {
        ($file:literal) => {
            concat!(
                include_str!("shaders/screen_common.wgsl"),
                "\n",
                include_str!($file)
            )
        };
    }

    /// 00, world vertex shader (CubeShader+0x04).
    pub const VS00_WORLD: &str = include_str!("shaders/vs00_world.wgsl");
    /// 01, world pixel shader: blocks, creatures, items, models (CubeShader+0x08).
    pub const PS01_WORLD: &str = world_ps!("shaders/ps01_world.wgsl");
    /// 02, sky background (CubeShader+0x0c).
    pub const PS02_SKY: &str = world_ps!("shaders/ps02_sky.wgsl");
    /// 03, water (CubeShader+0x14).
    pub const PS03_WATER: &str = world_ps!("shaders/ps03_water.wgsl");
    /// 04, never bound in the original (CubeShader+0x18).
    pub const PS04_UNUSED: &str = world_ps!("shaders/ps04_unused.wgsl");
    /// 05, clouds (CubeShader+0x10).
    pub const PS05_CLOUDS: &str = world_ps!("shaders/ps05_clouds.wgsl");
    /// 06, GUI vertex shader (D3D9Engine+0x194).
    pub const VS06_GUI: &str = include_str!("shaders/vs06_gui.wgsl");
    /// 07, GUI pixel shader (D3D9Engine+0x198).
    pub const PS07_GUI: &str = include_str!("shaders/ps07_gui.wgsl");
    /// 08, screen-quad vertex shader (D3D9Engine+0x1a0).
    pub const VS08_SCREEN: &str = include_str!("shaders/vs08_screen.wgsl");
    /// 09, blit (D3D9Engine+0x1a4).
    pub const PS09_BLIT: &str = screen_ps!("shaders/ps09_blit.wgsl");
    /// 10, 2x2 box downsample (D3D9Engine+0x1a8).
    pub const PS10_DOWNSAMPLE2: &str = screen_ps!("shaders/ps10_downsample2.wgsl");
    /// 11, 3x3 box downsample (D3D9Engine+0x1ac).
    pub const PS11_DOWNSAMPLE3: &str = screen_ps!("shaders/ps11_downsample3.wgsl");
    /// 12, 4x4 box downsample (D3D9Engine+0x1b0).
    pub const PS12_DOWNSAMPLE4: &str = screen_ps!("shaders/ps12_downsample4.wgsl");
    /// 13, vertical blur (D3D9Engine+0x1b8, ps_2_0).
    pub const PS13_BLUR_V: &str = screen_ps!("shaders/ps13_blur_v.wgsl");
    /// 14, horizontal blur (D3D9Engine+0x1b4).
    pub const PS14_BLUR_H: &str = screen_ps!("shaders/ps14_blur_h.wgsl");

    /// All fifteen, in the order of `analysis/shaders/manifest.json`.
    pub const SHADERS: [ShaderInfo; 15] = [
        ShaderInfo { index: 0, name: "world_vs", rva: 0x2fd928, stage: Stage::Vertex, source: VS00_WORLD },
        ShaderInfo { index: 1, name: "world_ps", rva: 0x2feb50, stage: Stage::Fragment, source: PS01_WORLD },
        ShaderInfo { index: 2, name: "sky_ps", rva: 0x2fefa0, stage: Stage::Fragment, source: PS02_SKY },
        ShaderInfo { index: 3, name: "water_ps", rva: 0x2ff1d0, stage: Stage::Fragment, source: PS03_WATER },
        ShaderInfo { index: 4, name: "unused_ps", rva: 0x2ff5c0, stage: Stage::Fragment, source: PS04_UNUSED },
        ShaderInfo { index: 5, name: "clouds_ps", rva: 0x2ff7c8, stage: Stage::Fragment, source: PS05_CLOUDS },
        ShaderInfo { index: 6, name: "gui_vs", rva: 0x320558, stage: Stage::Vertex, source: VS06_GUI },
        ShaderInfo { index: 7, name: "gui_ps", rva: 0x320e28, stage: Stage::Fragment, source: PS07_GUI },
        ShaderInfo { index: 8, name: "screen_vs", rva: 0x3212c0, stage: Stage::Vertex, source: VS08_SCREEN },
        ShaderInfo { index: 9, name: "blit_ps", rva: 0x321420, stage: Stage::Fragment, source: PS09_BLIT },
        ShaderInfo { index: 10, name: "downsample2_ps", rva: 0x321530, stage: Stage::Fragment, source: PS10_DOWNSAMPLE2 },
        ShaderInfo { index: 11, name: "downsample3_ps", rva: 0x321778, stage: Stage::Fragment, source: PS11_DOWNSAMPLE3 },
        ShaderInfo { index: 12, name: "downsample4_ps", rva: 0x321a98, stage: Stage::Fragment, source: PS12_DOWNSAMPLE4 },
        ShaderInfo { index: 13, name: "blur_v_ps", rva: 0x321f00, stage: Stage::Fragment, source: PS13_BLUR_V },
        ShaderInfo { index: 14, name: "blur_h_ps", rva: 0x322270, stage: Stage::Fragment, source: PS14_BLUR_H },
    ];
}

/// Drive a wgpu future to completion on the current thread.
///
/// wgpu-core's native futures (adapter/device requests, error scopes) are ready on the
/// first poll, so no executor dependency is needed; this spins with `yield_now` otherwise.
pub fn block_on<F: std::future::Future>(future: F) -> F::Output {
    use std::task::{Context, Poll, Waker};
    let mut future = std::pin::pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(v) = future.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::yield_now();
    }
}
