//! Port-only (not in Cube.exe): a pass drawn over the finished frame, after the GUI stream and
//! the multisample resolve, before the submit and present. The client's debug overlay
//! (`cw-client` `debug_overlay.rs`, egui) implements it; the renderer stays free of egui.
//!
//! [`crate::gpu::exec::Executor::render_with_overlay`] calls [`OverlayPass::encode`] only when
//! an overlay is given, so the original frame is unchanged without one.

/// The back buffer an overlay draws onto. It already holds the frame: an overlay's render pass
/// must load it (`LoadOp::Load`), never clear it. Single-sampled, no depth.
#[derive(Debug)]
pub struct OverlayTarget<'a> {
    /// The surface texture's view (the resolve target when anti-aliasing is on).
    pub view: &'a wgpu::TextureView,
    /// Its format ([`crate::gpu::device::GpuContext::color_format`], a non-sRGB unorm).
    pub format: wgpu::TextureFormat,
    /// Its size in physical pixels.
    pub width: u32,
    pub height: u32,
}

/// A pass drawn last onto the frame.
pub trait OverlayPass {
    /// Records the overlay into `encoder` (the frame's own, submitted right after). Returns
    /// command buffers that must be submitted before it (egui's paint callbacks; usually none).
    fn encode(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, target: &OverlayTarget<'_>) -> Vec<wgpu::CommandBuffer>;
}
