//! Cube World Alpha vector GUI (Cube.exe `plasma::*` and `cube::*Widget`).
//!
//! - [`drawing`]: `plasma::Drawing` CPU side, the 48-byte GUI vertex, tessellation and
//!   the anti-aliasing fringe (`Drawing::tessellate` 0x00675690,
//!   `tessellateWithFringe` 0x00673090).
//! - [`shape`]: `plasma::Shape` / `SmoothMeshShape` (keyframed contours, subdivision,
//!   building the fill/stroke/extrusion drawings).
//! - [`stroke`]: the stroke geometry of `SmoothMeshShape` (joints, caps, dashes).
//! - [`widget`]: `plasma::Widget` and its framework and game subclasses.
//! - [`loader`]: the scene a `.plx` document builds (`PlxReader::read` 0x00681c70):
//!   nodes, widgets, transformations, displays, shapes, textures, and their animation.
//! - [`font`]: `plasma::Font` / `FontEngine` / `ScalableFont`, glyph layout.
//! - [`render`]: `plasma::Engine::render` 0x00650980 and the `D3D9Engine` draw path: the
//!   GUI command stream the wgpu executor consumes, and the glyph atlas.

pub mod drawing;
pub mod font;
pub mod loader;
pub mod render;
pub mod shape;
pub mod stroke;
pub mod widget;
