//! `plasma::D3D9Texture` (vtable 0x00722608, size 0x60): texture creation and sampler state.
//!
//! The original uploads with `D3D9Texture::upload` 0x0068bde0: CreateTexture A8R8G8B8
//! (4 channels) or R8G8B8 (3 channels), MANAGED, one level unless the minification mode is
//! 2 (then a full chain is requested but only level 0 is filled), with an RGBA -> BGRA byte
//! swizzle. The port uploads `Rgba8Unorm` with one level (non-sRGB: D3D9 sampled gamma
//! values as they are), which samples identically.
//!
//! Premultiplied alpha: the GUI and render-surface pixel shaders (07 mask, 09..12) divide
//! rgb by alpha before blending with SRCALPHA/INVSRCALPHA, so they expect premultiplied
//! texels. Render surfaces are premultiplied by construction (the GUI blend writes alpha
//! with ONE/INVSRCALPHA). For image files the upload itself does not premultiply
//! (0x0068bde0 only swizzles), so [`premultiply_rgba8`] is offered to the caller who knows
//! the image's convention; see the report / TODO for the open question.

use wgpu::util::DeviceExt;

/// A texture and its default view (`plasma::D3D9Texture`).
#[derive(Debug)]
pub struct GpuTexture {
    /// The texture.
    pub texture: wgpu::Texture,
    /// Full view.
    pub view: wgpu::TextureView,
    /// Width in texels (D3D9Texture+0x44).
    pub width: u32,
    /// Height in texels (D3D9Texture+0x48).
    pub height: u32,
}

/// Colour format of every texture and render surface: non-sRGB RGBA8 (the D3D9 A8R8G8B8).
pub const TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

impl GpuTexture {
    /// Upload tightly packed RGBA8 texels (`width * height * 4` bytes).
    ///
    /// # Panics
    /// If `rgba.len()` is not `width * height * 4`.
    pub fn from_rgba8(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        label: &str,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> Self {
        assert_eq!(rgba.len(), width as usize * height as usize * 4, "RGBA8 size mismatch");
        let texture = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: TEXTURE_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            rgba,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self { texture, view, width, height }
    }

    /// Upload tightly packed RGB8 texels, expanded to RGBA8 with alpha 255.
    ///
    /// The original's 3-channel path memcpys RGB bytes into a D3DFMT_R8G8B8 texture, whose
    /// memory order is B, G, R, which would swap red and blue (and R8G8B8 textures are
    /// unsupported on most D3D9 hardware, so CreateTexture probably failed). The port keeps
    /// the channels as given; see the report.
    pub fn from_rgb8(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        label: &str,
        width: u32,
        height: u32,
        rgb: &[u8],
    ) -> Self {
        let rgba = rgb8_to_rgba8(rgb);
        Self::from_rgba8(device, queue, label, width, height, &rgba)
    }

    /// Decode a PNG with `image` and upload it, premultiplying when `premultiply` is set.
    pub fn from_png(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        label: &str,
        png: &[u8],
        premultiply: bool,
    ) -> Result<Self, image::ImageError> {
        let (width, height, mut rgba) = decode_png(png)?;
        if premultiply {
            premultiply_rgba8(&mut rgba);
        }
        Ok(Self::from_rgba8(device, queue, label, width, height, &rgba))
    }
}

/// Decode a PNG to (width, height, RGBA8 bytes).
pub fn decode_png(png: &[u8]) -> Result<(u32, u32, Vec<u8>), image::ImageError> {
    let img = image::load_from_memory_with_format(png, image::ImageFormat::Png)?.to_rgba8();
    let (w, h) = img.dimensions();
    Ok((w, h, img.into_raw()))
}

/// Expand RGB8 to RGBA8 with opaque alpha.
pub fn rgb8_to_rgba8(rgb: &[u8]) -> Vec<u8> {
    rgb.as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect()
}

/// Multiply rgb by alpha in place, rounding to nearest (`(c * a + 127) / 255`).
pub fn premultiply_rgba8(rgba: &mut [u8]) {
    for p in rgba.as_chunks_mut::<4>().0 {
        let a = u32::from(p[3]);
        for c in &mut p[..3] {
            *c = ((u32::from(*c) * a + 127) / 255) as u8;
        }
    }
}

/// Minification/magnification mode of a `plasma::Texture` (D3D9Texture+0x34 / +0x38).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TextureFilter {
    /// 0: D3DTEXF_POINT, no mip filter.
    #[default]
    Point,
    /// 1: D3DTEXF_LINEAR, no mip filter.
    Linear,
    /// 2: D3DTEXF_ANISOTROPIC with D3DTEXF_LINEAR mips. MAXANISOTROPY stays at its default 1
    /// and only level 0 exists, so this samples as Linear.
    Anisotropic,
}

/// Addressing mode of a `plasma::Texture` (D3D9Texture+0x3c for u, +0x40 for v).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TextureAddress {
    /// 0: D3DTADDRESS_BORDER (border colour default 0x00000000).
    #[default]
    Border,
    /// 1: D3DTADDRESS_WRAP.
    Wrap,
    /// 2: D3DTADDRESS_MIRROR.
    Mirror,
}

/// The sampler state `D3D9Texture::bind` 0x0068bc90 sets for one texture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TextureSampling {
    /// MINFILTER (+MIPFILTER).
    pub min: TextureFilter,
    /// MAGFILTER.
    pub mag: TextureFilter,
    /// ADDRESSU.
    pub address_u: TextureAddress,
    /// ADDRESSV.
    pub address_v: TextureAddress,
}

fn filter_mode(f: TextureFilter) -> wgpu::FilterMode {
    match f {
        TextureFilter::Point => wgpu::FilterMode::Nearest,
        TextureFilter::Linear | TextureFilter::Anisotropic => wgpu::FilterMode::Linear,
    }
}

/// Map an address mode. BORDER needs `Features::ADDRESS_MODE_CLAMP_TO_BORDER`; without it
/// the port falls back to ClampToEdge (edge texels instead of transparent black outside
/// [0, 1]).
fn address_mode(a: TextureAddress, border_supported: bool) -> wgpu::AddressMode {
    match a {
        TextureAddress::Border if border_supported => wgpu::AddressMode::ClampToBorder,
        TextureAddress::Border => wgpu::AddressMode::ClampToEdge,
        TextureAddress::Wrap => wgpu::AddressMode::Repeat,
        TextureAddress::Mirror => wgpu::AddressMode::MirrorRepeat,
    }
}

/// Create the sampler for a texture's [`TextureSampling`].
pub fn create_sampler(device: &wgpu::Device, s: TextureSampling) -> wgpu::Sampler {
    let border = device.features().contains(wgpu::Features::ADDRESS_MODE_CLAMP_TO_BORDER);
    let u = address_mode(s.address_u, border);
    let v = address_mode(s.address_v, border);
    let uses_border = u == wgpu::AddressMode::ClampToBorder || v == wgpu::AddressMode::ClampToBorder;
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("plasma texture"),
        address_mode_u: u,
        address_mode_v: v,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: filter_mode(s.mag),
        min_filter: filter_mode(s.min),
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        border_color: uses_border.then_some(wgpu::SamplerBorderColor::TransparentBlack),
        ..Default::default()
    })
}

/// The fixed samplers of the render-surface draws.
#[derive(Debug)]
pub struct Samplers {
    /// drawCopy 0x0068c7c0 (MIN/MAG LINEAR; addressing not set there, clamp assumed) and
    /// drawBlurred 0x0068cad0 (LINEAR, ADDRESSU/V CLAMP).
    pub linear_clamp: wgpu::Sampler,
    /// drawScaled 0x0068cf10 (MIN/MAG POINT).
    pub point_clamp: wgpu::Sampler,
    /// A placeholder for an unbound GUI mask/texture slot.
    pub dummy_texture: wgpu::TextureView,
}

impl Samplers {
    /// Create the fixed samplers and a 1x1 transparent texture for unused slots.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let clamp = |filter| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("render surface"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: filter,
                min_filter: filter,
                ..Default::default()
            })
        };
        let dummy = GpuTexture::from_rgba8(device, queue, "dummy", 1, 1, &[0, 0, 0, 0]);
        Self {
            linear_clamp: clamp(wgpu::FilterMode::Linear),
            point_clamp: clamp(wgpu::FilterMode::Nearest),
            dummy_texture: dummy.view,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn premultiply_rounds() {
        let mut px = [255, 128, 0, 128, 10, 20, 30, 255, 200, 200, 200, 0];
        premultiply_rgba8(&mut px);
        assert_eq!(px, [128, 64, 0, 128, 10, 20, 30, 255, 0, 0, 0, 0]);
    }

    #[test]
    fn rgb_expands() {
        assert_eq!(rgb8_to_rgba8(&[1, 2, 3, 4, 5, 6]), vec![1, 2, 3, 255, 4, 5, 6, 255]);
    }
}
