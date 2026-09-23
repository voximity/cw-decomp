//! wgpu instance, adapter, device, queue and the window surface: the replacement for the
//! IDirect3D9 / IDirect3DDevice9 pair of Cube.exe (CreateDevice through IDirect3D9 +0x40,
//! Present +0x44 in 0x004c85f0, Reset +0x40 in 0x004c8940 on resize / device loss).
//!
//! Backends: Vulkan on Windows and Linux, Metal on macOS (the platform's primary modern API);
//! `WGPU_BACKEND` overrides it (`InstanceDescriptor::with_env`).
//!
//! sRGB: the original never converts to or from sRGB. The surface is configured with a
//! non-sRGB format when the platform offers one; when it only offers sRGB formats, the
//! surface texture is viewed through its non-sRGB twin (`view_formats`), so shaders always
//! write gamma-space values straight through, as D3D9 did. [`GpuContext::color_format`] is
//! the format every pipeline and render surface uses.

use super::block_on;
use super::targets::create_depth_texture_ms;

/// Why GPU start-up failed.
#[derive(Debug)]
pub enum GpuError {
    /// `Instance::create_surface` failed.
    Surface(wgpu::CreateSurfaceError),
    /// No adapter matched.
    Adapter(wgpu::RequestAdapterError),
    /// The device request failed.
    Device(wgpu::RequestDeviceError),
    /// The surface reports no usable format for this adapter.
    Unsupported,
}

impl std::fmt::Display for GpuError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GpuError::Surface(e) => write!(f, "cannot create the window surface: {e}"),
            GpuError::Adapter(e) => write!(f, "no suitable GPU adapter: {e}"),
            GpuError::Device(e) => write!(f, "cannot open the GPU device: {e}"),
            GpuError::Unsupported => write!(f, "the window surface is not supported by the adapter"),
        }
    }
}

impl std::error::Error for GpuError {}

/// Backends to try: Vulkan on Windows/Linux, Metal on macOS, the primary set elsewhere.
pub fn default_backends() -> wgpu::Backends {
    if cfg!(target_os = "macos") || cfg!(target_os = "ios") {
        wgpu::Backends::METAL
    } else if cfg!(any(target_os = "windows", target_os = "linux")) {
        wgpu::Backends::VULKAN
    } else {
        wgpu::Backends::PRIMARY
    }
}

/// Create an instance on [`default_backends`], overridable through the environment.
pub fn create_instance() -> wgpu::Instance {
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = default_backends();
    wgpu::Instance::new(desc.with_env())
}

/// Features requested when the adapter has them: ClampToBorder for the GUI's BORDER
/// addressing (beginFrame 0x00688b60 sets s1 ADDRESSU/V = BORDER).
fn wanted_features(adapter: &wgpu::Adapter) -> wgpu::Features {
    // TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES: sample counts other than 1 and 4 for the
    // anti-aliasing option ([`GpuContext::set_sample_count`]).
    adapter.features() & (wgpu::Features::ADDRESS_MODE_CLAMP_TO_BORDER | wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES)
}

/// The largest sample count `<= requested` that `format` and [`super::pipelines::DEPTH_FORMAT`]
/// both support as render attachments on this device (1 when `requested <= 1`). Without
/// `TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES` only 1 and 4 are guaranteed.
pub fn supported_sample_count(adapter: &wgpu::Adapter, device: &wgpu::Device, format: wgpu::TextureFormat, requested: u32) -> u32 {
    if requested <= 1 {
        return 1;
    }
    let specific = device.features().contains(wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES);
    let ok = |n: u32| {
        if !specific {
            return n == 4;
        }
        let c = adapter.get_texture_format_features(format).flags;
        let d = adapter.get_texture_format_features(super::pipelines::DEPTH_FORMAT).flags;
        c.sample_count_supported(n) && d.sample_count_supported(n)
    };
    [16u32, 8, 4, 2].into_iter().find(|&n| n <= requested && ok(n)).unwrap_or(1)
}

/// Request a device with downlevel limits (the renderer needs little) but the adapter's
/// full 2D texture size, for large windows.
pub fn request_device(adapter: &wgpu::Adapter) -> Result<(wgpu::Device, wgpu::Queue), wgpu::RequestDeviceError> {
    let limits = wgpu::Limits {
        max_texture_dimension_2d: adapter.limits().max_texture_dimension_2d,
        ..wgpu::Limits::downlevel_defaults()
    };
    block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("cw-render"),
        required_features: wanted_features(adapter),
        required_limits: limits,
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
}

/// A device without a window (tests, tools). `None` when no adapter is available.
pub fn headless() -> Option<(wgpu::Instance, wgpu::Adapter, wgpu::Device, wgpu::Queue)> {
    let instance = create_instance();
    let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
        apply_limit_buckets: false,
    }))
    .ok()?;
    let (device, queue) = request_device(&adapter).ok()?;
    Some((instance, adapter, device, queue))
}

/// The GPU and the window surface (the original's IDirect3DDevice9 + swap chain + the
/// back buffer's D24S8 depth surface).
#[derive(Debug)]
pub struct GpuContext<'w> {
    /// Instance.
    pub instance: wgpu::Instance,
    /// Adapter.
    pub adapter: wgpu::Adapter,
    /// Device.
    pub device: wgpu::Device,
    /// Queue.
    pub queue: wgpu::Queue,
    /// Window surface.
    pub surface: wgpu::Surface<'w>,
    /// Current surface configuration.
    pub config: wgpu::SurfaceConfiguration,
    /// Non-sRGB format the surface texture is viewed and rendered as.
    color_format: wgpu::TextureFormat,
    /// Depth attachment of the back buffer.
    pub depth: wgpu::Texture,
    /// View of [`Self::depth`].
    pub depth_view: wgpu::TextureView,
    /// Samples per pixel of the back buffer (1 = no anti-aliasing).
    sample_count: u32,
    /// The multisampled colour target the frame is drawn into and resolved from into the
    /// surface texture, when [`GpuContext::sample_count`] > 1.
    pub msaa_view: Option<wgpu::TextureView>,
    /// Set to read the next presented back buffer back into [`Self::captured`] (the port's
    /// screenshot facility for smoke runs; needs a surface with `COPY_SRC`).
    pub capture_requested: bool,
    /// The last captured back buffer: `(width, height, RGBA8 rows)`.
    pub captured: Option<(u32, u32, Vec<u8>)>,
}

/// One acquired back buffer.
#[derive(Debug)]
pub struct Frame {
    /// The surface texture; hand back to [`GpuContext::present`].
    pub texture: wgpu::SurfaceTexture,
    /// Its view in [`GpuContext::color_format`].
    pub view: wgpu::TextureView,
}

impl<'w> GpuContext<'w> {
    /// Create everything for a window. `target` is anything wgpu accepts as a surface target,
    /// e.g. an `Arc<winit::window::Window>` (it implements `HasWindowHandle` and
    /// `HasDisplayHandle`). `width`/`height` are the inner size in physical pixels.
    /// `vsync` picks FIFO; otherwise `AutoNoVsync` (the original's FPS limit lives in the
    /// frame loop).
    pub fn new(
        target: impl Into<wgpu::SurfaceTarget<'w>>,
        width: u32,
        height: u32,
        vsync: bool,
    ) -> Result<Self, GpuError> {
        let instance = create_instance();
        let surface = instance.create_surface(target).map_err(GpuError::Surface)?;
        let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
            apply_limit_buckets: false,
        }))
        .map_err(GpuError::Adapter)?;
        let (device, queue) = request_device(&adapter).map_err(GpuError::Device)?;

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb() && matches!(f, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm))
            .or_else(|| caps.formats.first().copied())
            .ok_or(GpuError::Unsupported)?;
        let color_format = format.remove_srgb_suffix();
        let alpha_mode = if caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::Opaque) {
            wgpu::CompositeAlphaMode::Opaque
        } else {
            caps.alpha_modes.first().copied().unwrap_or(wgpu::CompositeAlphaMode::Auto)
        };
        // COPY_SRC where the surface allows it, for [`GpuContext::capture_requested`].
        let usage = wgpu::TextureUsages::RENDER_ATTACHMENT | (caps.usages & wgpu::TextureUsages::COPY_SRC);
        let config = wgpu::SurfaceConfiguration {
            usage,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: width.max(1),
            height: height.max(1),
            present_mode: if vsync { wgpu::PresentMode::Fifo } else { wgpu::PresentMode::AutoNoVsync },
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: if color_format != format { vec![color_format] } else { vec![] },
        };
        surface.configure(&device, &config);
        let depth = create_depth_texture_ms(&device, config.width, config.height, 1);
        let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
        Ok(Self { instance, adapter, device, queue, surface, config, color_format, depth, depth_view, sample_count: 1, msaa_view: None, capture_requested: false, captured: None })
    }

    /// The non-sRGB colour format to build pipelines and render surfaces with.
    pub fn color_format(&self) -> wgpu::TextureFormat {
        self.color_format
    }

    /// Reconfigure after a window resize (the original Resets the device, 0x004c8940).
    /// A zero size (minimised window) is ignored.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.create_targets();
    }

    /// Samples per pixel of the back buffer.
    pub fn sample_count(&self) -> u32 {
        self.sample_count
    }

    /// The anti-aliasing option (`0x0076b1e4`; `resetDevice` 0x004c8940 asks D3D9 for
    /// `value * 2` samples, 0 = none): `requested` samples, lowered to what the adapter
    /// supports ([`supported_sample_count`]). Recreates the depth and multisampled colour
    /// targets; returns the count in use. Pass the result to
    /// [`super::exec::Executor::set_sample_count`].
    pub fn set_sample_count(&mut self, requested: u32) -> u32 {
        let n = supported_sample_count(&self.adapter, &self.device, self.color_format, requested);
        if n != self.sample_count {
            self.sample_count = n;
            self.create_targets();
        }
        n
    }

    /// The back buffer's depth (and multisampled colour) targets at the current size and
    /// sample count.
    fn create_targets(&mut self) {
        let (w, h) = (self.config.width, self.config.height);
        self.depth = create_depth_texture_ms(&self.device, w, h, self.sample_count);
        self.depth_view = self.depth.create_view(&wgpu::TextureViewDescriptor::default());
        self.msaa_view = (self.sample_count > 1).then(|| {
            self.device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("msaa colour"),
                    size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: self.sample_count,
                    dimension: wgpu::TextureDimension::D2,
                    format: self.color_format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        });
    }

    /// Acquire the next back buffer. `None` means skip this frame (timeout, occluded,
    /// outdated or lost surface; the surface is reconfigured where that helps).
    pub fn acquire(&mut self) -> Option<Frame> {
        let texture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) => t,
            wgpu::CurrentSurfaceTexture::Suboptimal(t) => {
                // Use this one; reconfigure before the next.
                let frame = self.frame_from(t);
                self.surface.configure(&self.device, &self.config);
                return Some(frame);
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return None;
            }
            _ => return None,
        };
        Some(self.frame_from(texture))
    }

    fn frame_from(&self, texture: wgpu::SurfaceTexture) -> Frame {
        let view = texture.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(self.color_format),
            ..Default::default()
        });
        Frame { texture, view }
    }

    /// Reads `frame` back into [`Self::captured`] (after its work was submitted) when a
    /// capture was requested and the surface allows copies; clears the request.
    pub fn capture_if_requested(&mut self, frame: &Frame) {
        if !self.capture_requested || !self.config.usage.contains(wgpu::TextureUsages::COPY_SRC) {
            return;
        }
        self.capture_requested = false;
        let (w, h) = (self.config.width, self.config.height);
        let row = (w * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("capture"),
            size: u64::from(row) * u64::from(h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("capture") });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &frame.texture.texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) } },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.queue.submit([enc.finish()]);
        buf.map_async(wgpu::MapMode::Read, .., |_| {});
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        let Ok(view) = buf.get_mapped_range(..) else { return };
        let bgra = matches!(self.config.format.remove_srgb_suffix(), wgpu::TextureFormat::Bgra8Unorm);
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h as usize {
            let r = &view[y * row as usize..y * row as usize + w as usize * 4];
            for px in r.chunks_exact(4) {
                if bgra {
                    out.extend_from_slice(&[px[2], px[1], px[0], 255]);
                } else {
                    out.extend_from_slice(&[px[0], px[1], px[2], 255]);
                }
            }
        }
        drop(view);
        buf.unmap();
        self.captured = Some((w, h, out));
    }

    /// Present (IDirect3DDevice9::Present, 0x004c85f0). Submit the frame's work first.
    pub fn present(&self, frame: Frame) {
        drop(frame.view);
        self.queue.present(frame.texture);
    }
}
