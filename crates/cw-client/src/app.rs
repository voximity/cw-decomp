//! The window and the frame loop: `WinMain` 0x004c8ae0 (window, device, options, audio,
//! input devices, the controller), its message loop 0x004c9057, `WndProc` 0x004c83f0 and
//! `frame` 0x004c85f0, on winit 0.30 (`ApplicationHandler`), gilrs and the wgpu executor.
//!
//! Tier C for the windowing and the devices; the frame order is the original's:
//!
//! ```text
//! loop:                                   // 0x004c9057
//!   t0 = timeGetTime()
//!   messages (WndProc: keys, chars, buttons, wheel, size)       -> window_event
//!   quit flag (GC+0x1a0) -> PostQuitMessage                     -> ActiveEventLoop::exit
//!   focused: cursor recentred for mouse look (SetCursorPos) or  -> cursor grab
//!            released; keyboard/mouse device state into the
//!            controller bytes; slot 6 onMouseMove(dx, dy)/(x, y)
//!   frame():  dt = now - last; update(dt); options changed ->   -> Controller::update,
//!             resetDevice; render(); Present                        Controller::render
//!   sleep(minTimeStep - (now - t0)) when positive               -> the FPS limit
//! ```
//!
//! DPI: Cube.exe has no `dpiAware` manifest entry and never calls `SetProcessDPIAware`, so on a
//! scaled monitor Windows virtualises it: its window, `GetSystemMetrics`, the cursor and the
//! D3D9 back buffer are in logical pixels (physical / scale) and DWM stretches the result.
//! The GUI (projection of `beginFrame` 0x00688b60 over `Engine+0x10c/+0x110`, one unit = one
//! back-buffer pixel; `onResize` 0x00482a40 anchoring in those pixels) therefore appears
//! `scale` times larger than its pixel sizes. winit makes the port per-monitor DPI aware, so
//! the port keeps that look by giving the controller the logical client size and cursor
//! ([`App::gui_scale`]) while the back buffer stays physical: the 3D scene is drawn at full
//! resolution and the GUI, laid out in logical pixels, is drawn scaled to the back buffer
//! (`Controller::set_back_buffer`, `cw_ui::render::GuiView::scale`) rather than stretched
//! from a logical-size image, so its text is rasterised at the physical size (DWM's stretch,
//! and a stretched stream, blur it). In fullscreen `resetDevice` 0x004c8940 sizes the back
//! buffer, and so the GUI's pixel space, to the resolution option and the monitor scales that
//! mode to the screen; the port lays the GUI out at the resolution and draws it at the
//! fullscreen window's physical size, fitted to the screen ([`gui_layout`]). The resolution
//! option, its default and the mode list are in the virtualised process's logical units
//! ([`logical_display_modes`]), so the default fullscreen GUI is the size of the windowed one.
//!
//! Differences (Tier C): the controller bytes are cleared when the window loses the focus
//! (the original keeps the last DirectInput state, which leaves keys held); the first
//! frame's `dt` is measured from the start of the loop (the original's `last` starts at 0,
//! which makes its first `dt` the system uptime); gamepad sticks fill the axes the original
//! leaves at zero (`input.rs`); the free cursor follows `CursorMoved` without the focus (the
//! original moves it only while focused, but macOS can leave a terminal-launched window
//! unfocused).

#![allow(dead_code)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use cw_render::frame::{FrameCommands, Geometry};
use cw_render::gpu::device::GpuContext;
use cw_render::gpu::exec::{Executor, Resources};
use cw_render::mesh::{VoxelGrid, build_model_mesh, build_world_model_mesh};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use crate::controller::{Controller, FrameResources, FrameSink, PARTICLE_CUBE, default_game_dir};
use crate::input::{MouseButton, dik, vk};
use crate::profile::{self, FrameLog, Phase};

// ---------------------------------------------------------------------------------------------
// The device sinks.

/// The wgpu executor as the controller's [`FrameSink`]: `D3D9Engine`/`CubeShader` replaced by
/// `cw_render::gpu::exec`. It keeps the GPU copies of what the frame names: the chunk
/// buffers by ring index (re-uploaded when a record's version changes), the model meshes
/// (built on first use from the model table), the map tile meshes.
pub struct ExecSink {
    pub ctx: GpuContext<'static>,
    pub exec: Executor,
    pub res: Resources,
    chunk_versions: Vec<u64>,
    /// Port-only debug overlay (`debug_overlay.rs`): the egui renderer.
    #[cfg(feature = "debug-overlay")]
    pub overlay: crate::debug_overlay::OverlayRenderer,
}

impl ExecSink {
    pub fn new(window: Arc<Window>) -> Result<ExecSink, String> {
        let size = window.inner_size();
        let ctx = GpuContext::new(window, size.width, size.height, false).map_err(|e| e.to_string())?;
        let exec = Executor::new(&ctx.device, &ctx.queue, ctx.color_format());
        let res = Resources::new(&ctx.device, exec.color_format());
        Ok(ExecSink { ctx, exec, res, chunk_versions: Vec::new(),
            #[cfg(feature = "debug-overlay")]
            overlay: Default::default(),
        })
    }

    fn sync(&mut self, frame: &FrameCommands, r: &FrameResources<'_>) {
        let dev = &self.ctx.device;
        if self.chunk_versions.len() != r.chunks.len() {
            self.chunk_versions = vec![u64::MAX; r.chunks.len()];
        }
        for (i, (v, b)) in r.chunks.iter().enumerate() {
            if self.chunk_versions[i] == *v {
                continue;
            }
            self.chunk_versions[i] = *v;
            match b {
                Some(b) => self.res.upload_chunk(dev, i, b),
                None => self.res.remove_chunk(i),
            }
        }
        for (model, mesh) in r.map_meshes {
            match mesh {
                Some(m) => self.res.models.insert_mesh(dev, *model, m),
                None => self.res.models.remove(*model),
            }
        }
        for pass in &frame.passes {
            for d in &pass.draws {
                let Geometry::Model { model } = d.geometry else { continue };
                let models = r.models;
                self.res.models.ensure(dev, model, || {
                    if model == PARTICLE_CUBE {
                        // `GC+0x800730`: a 1x1x1 white voxel.
                        return Some(build_model_mesh(VoxelGrid { size: [1, 1, 1], voxels: &[[255, 255, 255]] }, [0, 0, 0], false));
                    }
                    let m = models.models.get(model as usize)?;
                    Some(build_world_model_mesh(m, [0, 0, 0]))
                });
            }
        }
    }
}

impl FrameSink for ExecSink {
    /// `--shot-at`: read the next presented frame back.
    fn request_capture(&mut self) {
        self.ctx.capture_requested = true;
    }

    fn take_capture(&mut self) -> Option<(u32, u32, Vec<u8>)> {
        self.ctx.captured.take()
    }

    // Port-only debug overlay (`debug_overlay.rs`).
    #[cfg(feature = "debug-overlay")]
    fn debug_overlay(&mut self) -> Option<&mut crate::debug_overlay::OverlayRenderer> {
        Some(&mut self.overlay)
    }

    fn render(&mut self, frame: &FrameCommands, mut r: FrameResources<'_>) {
        // The GUI stream (`Engine::render 0x00650980` through `D3D9Engine`) and the ctor's
        // assets first, so the tinted cloud mesh is in place before the lazy model builds.
        let uploads = profile::scope(Phase::Uploads);
        crate::assets::upload(&self.ctx, &mut self.res, std::mem::take(&mut r.upload));
        self.sync(frame, &r);
        drop(uploads);
        // Port-only debug overlay: drawn last when it has a paint (`debug_overlay.rs`).
        #[cfg(feature = "debug-overlay")]
        self.exec.render_with_overlay(&mut self.ctx, frame, &mut self.res, self.overlay.pass());
        #[cfg(not(feature = "debug-overlay"))]
        self.exec.render(&mut self.ctx, frame, &mut self.res);
        let t = self.exec.timings;
        profile::add(Phase::Acquire, t.acquire);
        profile::add(Phase::Encode, t.encode);
        profile::add(Phase::Submit, t.submit);
        profile::add(Phase::Present, t.present);
    }

    fn resize(&mut self, width: u32, height: u32) {
        self.ctx.resize(width, height);
    }

    fn set_anti_aliasing(&mut self, level: i32) {
        let n = self.ctx.set_sample_count((level.max(0) as u32) * 2);
        self.exec.set_sample_count(&self.ctx.device, n);
    }
}

/// A sink that draws nothing (no GPU adapter).
pub struct NullSink;

impl FrameSink for NullSink {
    fn render(&mut self, _frame: &FrameCommands, _r: FrameResources<'_>) {}
    fn resize(&mut self, _width: u32, _height: u32) {}
}

// ---------------------------------------------------------------------------------------------
// Key codes.

/// The DirectInput scan code of a key (the keyboard `GetDeviceState(256)` buffer index), for
/// the keys `WinMain` copies and the digits.
pub fn dik_of(k: KeyCode) -> Option<u8> {
    Some(match k {
        KeyCode::KeyW => dik::W,
        KeyCode::KeyS => dik::S,
        KeyCode::KeyA => dik::A,
        KeyCode::KeyD => dik::D,
        KeyCode::KeyE => dik::E,
        KeyCode::KeyR => dik::R,
        KeyCode::KeyT => dik::T,
        KeyCode::KeyQ => dik::Q,
        KeyCode::Space => dik::SPACE,
        KeyCode::ShiftLeft => dik::LSHIFT,
        KeyCode::ControlLeft => dik::LCONTROL,
        KeyCode::Digit1 => dik::KEY_1,
        KeyCode::Digit2 => dik::KEY_2,
        KeyCode::Digit3 => dik::KEY_3,
        KeyCode::Digit4 => dik::KEY_4,
        _ => return None,
    })
}

/// The Windows virtual-key code `WndProc` passes to `onKeyDown`/`onKeyUp`.
pub fn vk_of(k: KeyCode) -> Option<u8> {
    use KeyCode::*;
    let letter = |i: u8| Some(b'A' + i);
    match k {
        KeyA => letter(0),
        KeyB => letter(1),
        KeyC => letter(2),
        KeyD => letter(3),
        KeyE => letter(4),
        KeyF => letter(5),
        KeyG => letter(6),
        KeyH => letter(7),
        KeyI => letter(8),
        KeyJ => letter(9),
        KeyK => letter(10),
        KeyL => letter(11),
        KeyM => letter(12),
        KeyN => letter(13),
        KeyO => letter(14),
        KeyP => letter(15),
        KeyQ => letter(16),
        KeyR => letter(17),
        KeyS => letter(18),
        KeyT => letter(19),
        KeyU => letter(20),
        KeyV => letter(21),
        KeyW => letter(22),
        KeyX => letter(23),
        KeyY => letter(24),
        KeyZ => letter(25),
        Digit0 => Some(b'0'),
        Digit1 => Some(b'1'),
        Digit2 => Some(b'2'),
        Digit3 => Some(b'3'),
        Digit4 => Some(b'4'),
        Digit5 => Some(b'5'),
        Digit6 => Some(b'6'),
        Digit7 => Some(b'7'),
        Digit8 => Some(b'8'),
        Digit9 => Some(b'9'),
        Tab => Some(vk::TAB),
        Escape => Some(vk::ESCAPE),
        Enter | NumpadEnter => Some(vk::RETURN),
        Backspace => Some(0x08),
        Space => Some(0x20),
        ShiftLeft | ShiftRight => Some(0x10),
        ControlLeft | ControlRight => Some(0x11),
        AltLeft | AltRight => Some(0x12),
        ArrowLeft => Some(0x25),
        ArrowUp => Some(0x26),
        ArrowRight => Some(0x27),
        ArrowDown => Some(0x28),
        Delete => Some(0x2e),
        Home => Some(0x24),
        End => Some(0x23),
        F1 => Some(vk::F1),
        F2 => Some(0x71),
        F3 => Some(0x72),
        F4 => Some(0x73),
        F5 => Some(0x74),
        F6 => Some(0x75),
        F7 => Some(0x76),
        F8 => Some(0x77),
        F9 => Some(0x78),
        F10 => Some(0x79),
        F11 => Some(0x7a),
        F12 => Some(0x7b),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------
// The application.

/// What `WinMain` hands the constructor from the command line.
#[derive(Debug, Clone, Default)]
pub struct Args {
    /// argv[1] == "server" (0x004c8f9c): a flag the constructor never reads.
    pub server_flag: bool,
    /// Any other argv[1], narrowed: the preset server address (`GC+0x800598`).
    pub server_address: String,
    /// Stop after this many seconds (for smoke runs; not in the original).
    pub quit_after: Option<f32>,
    /// `--start-world <seed>:<name>`: go straight into a world, as the world select screen's
    /// "Select" does (`startWorld` 0x0046f620; not in the original).
    pub start_world: Option<(i32, String)>,
    /// `--key-at <seconds>:<vk hex>`, `--cursor-at <seconds>:<x>,<y>` and `--click-at
    /// <seconds>:<x>,<y>` (repeatable): input
    /// injected at those times of a smoke run (the port's own; for scripted screenshots).
    pub script: Vec<(f32, ScriptEvent)>,
}

/// One scripted input of [`Args::script`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScriptEvent {
    /// A key press and release (virtual-key code): slots 2 and 3.
    Key(u8),
    /// The cursor moved to a client position (slot 6 with the absolute position when free).
    Cursor(f32, f32),
    /// `--shot-at <seconds>`: the next presented frame is read back and saved as
    /// `shot_<n>.png` in the directory of `CW_SHOT_DIR` (else the working directory).
    Shot,
    /// `--click-at <seconds>:<x>,<y>`: the cursor moves to the client position (slot 6), then
    /// a left press and release (slots 7 and 8).
    Click(f32, f32),
    /// `--press-at <seconds>:<x>,<y>` / `--release-at <seconds>:<x>,<y>`: the cursor moves
    /// there, then only the left press (slot 7) or only the release (slot 8), for drags.
    Press(f32, f32),
    Release(f32, f32),
    /// `--text-at <seconds>:<text>`: each character as `WM_CHAR` (slot 5).
    Char(u16),
}

/// `WinMain`'s command line (`CommandLineToArgvW`): only argv[1] is looked at. `--quit-after
/// <seconds>` is the port's own.
pub fn parse_args(args: &[String]) -> Args {
    let mut a = Args::default();
    let mut rest: Vec<&String> = Vec::new();
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--quit-after" && i + 1 < args.len() {
            a.quit_after = args[i + 1].parse().ok();
            i += 2;
            continue;
        }
        if args[i] == "--shot-at" && i + 1 < args.len() {
            if let Ok(t) = args[i + 1].parse::<f32>() {
                a.script.push((t, ScriptEvent::Shot));
            }
            i += 2;
            continue;
        }
        if args[i] == "--text-at" && i + 1 < args.len() {
            if let Some((t, v)) = args[i + 1].split_once(':')
                && let Ok(t) = t.parse::<f32>()
            {
                for c in v.encode_utf16() {
                    a.script.push((t, ScriptEvent::Char(c)));
                }
            }
            i += 2;
            continue;
        }
        if matches!(args[i].as_str(), "--key-at" | "--cursor-at" | "--click-at" | "--press-at" | "--release-at") && i + 1 < args.len() {
            if let Some((t, v)) = args[i + 1].split_once(':')
                && let Ok(t) = t.parse::<f32>()
            {
                let ev = if args[i] == "--key-at" {
                    u8::from_str_radix(v.trim_start_matches("0x"), 16).ok().map(ScriptEvent::Key)
                } else {
                    let kind = args[i].clone();
                    v.split_once(',').and_then(|(x, y)| {
                        let (x, y) = (x.parse().ok()?, y.parse().ok()?);
                        Some(match kind.as_str() {
                            "--click-at" => ScriptEvent::Click(x, y),
                            "--press-at" => ScriptEvent::Press(x, y),
                            "--release-at" => ScriptEvent::Release(x, y),
                            _ => ScriptEvent::Cursor(x, y),
                        })
                    })
                };
                if let Some(ev) = ev {
                    a.script.push((t, ev));
                }
            }
            i += 2;
            continue;
        }
        if args[i] == "--start-world" && i + 1 < args.len() {
            if let Some((seed, name)) = args[i + 1].split_once(':') {
                a.start_world = seed.parse().ok().map(|s| (s, name.to_string()));
            }
            i += 2;
            continue;
        }
        rest.push(&args[i]);
        i += 1;
    }
    if let Some(first) = rest.first() {
        if first.as_str() == "server" {
            a.server_flag = true;
        } else {
            a.server_address = (*first).clone();
        }
    }
    a
}

/// The window, the controller, the devices and the loop state.
pub struct App {
    args: Args,
    window: Option<Arc<Window>>,
    controller: Option<Controller>,
    sink: Option<Box<dyn FrameSink>>,
    gilrs: Option<gilrs::Gilrs>,
    /// `0x00766218`: the window has the focus.
    focused: bool,
    /// The mouse motion of this frame (`DIMOUSESTATE::lX/lY`).
    mouse_delta: [f64; 2],
    /// The cursor in client coordinates (`GetCursorPos` + `ScreenToClient`).
    cursor: [f64; 2],
    /// `0x0076b1d0`: the cursor was put back once after mouse look ended.
    cursor_restored: bool,
    grabbed: bool,
    /// `0x0076b234`: `timeGetTime()` of the last `frame` (whole milliseconds since `started`).
    last: Option<u64>,
    started: Instant,
    /// `0x0076b1d8..0x0076b1e4`: the fullscreen, resolution and anti-aliasing options the
    /// device was last reset with (`frame` compares the controller's `+0x170..+0x17c` with
    /// them).
    device_options: Option<[i32; 4]>,
    /// Screenshots saved so far (`--shot-at`).
    shots: u32,
    /// `CW_CLIENT_STATS`: the end of the last frame and the frame-time log.
    frame_start: Option<Instant>,
    log: FrameLog,
    /// Port-only debug overlay (`debug_overlay.rs`, F3).
    #[cfg(feature = "debug-overlay")]
    debug: crate::debug_overlay::DebugOverlay,
}

/// A physical size divided by the GUI scale, at least 1 (see [`App::gui_scale`]).
fn logical_size(width: u32, height: u32, scale: f64) -> (i32, i32) {
    let l = |v: u32| ((f64::from(v) / scale).round() as i32).max(1);
    (l(width), l(height))
}

/// The GUI (client) size the controller gets (`GC+0x11c/+0x120`) and the device pixels per
/// GUI unit ([`App::gui_scale`], `Controller::set_back_buffer`), for a back buffer of
/// `physical` pixels in a window of DPI scale `scale_factor`; `fullscreen` is the resolution
/// option (`0x0076b1dc`/`0x0076b1e0`) when fullscreen.
///
/// Windowed, the logical client size (the DPI virtualisation of the module doc). Fullscreen,
/// `resetDevice` 0x004c8940 makes the back buffer, and so the GUI's pixel space, the
/// resolution option, which the monitor then scales to the screen. The port's back buffer is
/// the fullscreen window's physical size (a borderless window at the native size, or the
/// exclusive mode's): the GUI is laid out at the resolution, fitted to the screen (the largest
/// uniform scale at which the resolution fits, so the GUI fills the screen and is at least the
/// resolution on both axes), and drawn crisply at that scale.
fn gui_layout(physical: (u32, u32), scale_factor: f64, fullscreen: Option<(i32, i32)>) -> ((i32, i32), f64) {
    let scale = match fullscreen {
        Some((rx, ry)) if rx > 0 && ry > 0 => (f64::from(physical.0) / f64::from(rx)).min(f64::from(physical.1) / f64::from(ry)),
        _ => scale_factor,
    }
    .max(1e-3);
    (logical_size(physical.0, physical.1, scale), scale)
}

/// `WinMain` 0x004c8ae0 step 5 (the unique width/height pairs of the adapter's modes) in the
/// units of the virtualised process: each mode divided by the monitor's DPI scale, so the
/// resolution option, its default (`GetSystemMetrics(0/1)`, virtualised to the logical screen
/// size) and the windowed client size share the GUI's units.
fn logical_display_modes(modes: impl IntoIterator<Item = (u32, u32)>, scale_factor: f64) -> Vec<(i32, i32)> {
    let mut out: Vec<(i32, i32)> = Vec::new();
    for (w, h) in modes {
        let p = logical_size(w, h, scale_factor.max(1e-3));
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

/// The device pixel size of a resolution in GUI units (the exclusive mode `resetDevice` looks
/// for): the inverse of [`logical_display_modes`].
fn physical_mode_size(resolution: (i32, i32), scale_factor: f64) -> (u32, u32) {
    let p = |v: i32| (f64::from(v.max(0)) * scale_factor).round() as u32;
    (p(resolution.0), p(resolution.1))
}

/// The slot 6 `onMouseMove` argument of a frame: the absolute client position when the
/// cursor is free, the relative motion (mouse look) when it is not and the window has the
/// focus.
fn slot6_motion(free: bool, focused: bool, cursor: [f64; 2], delta: [f64; 2]) -> Option<(f32, f32)> {
    if free {
        Some((cursor[0] as f32, cursor[1] as f32))
    } else if focused {
        Some((delta[0] as f32, delta[1] as f32))
    } else {
        None
    }
}

/// Where the free cursor is taken when it leaves the window: off the client area (so no node
/// is under it), unless a mouse button is held (the drag's own `CursorMoved`s follow it).
fn cursor_after_leave(cursor: [f64; 2], buttons: [bool; 3]) -> [f64; 2] {
    if buttons.iter().any(|&b| b) { cursor } else { [-1.0, -1.0] }
}

impl App {
    pub fn new(args: Args) -> App {
        App {
            args,
            window: None,
            controller: None,
            sink: None,
            gilrs: gilrs::Gilrs::new().ok(),
            focused: true,
            mouse_delta: [0.0; 2],
            cursor: [0.0; 2],
            cursor_restored: true,
            grabbed: false,
            last: None,
            started: Instant::now(),
            device_options: None,
            shots: 0,
            frame_start: None,
            log: FrameLog::default(),
            #[cfg(feature = "debug-overlay")]
            debug: crate::debug_overlay::DebugOverlay::default(),
        }
    }

    /// The factor between the back buffer and the size the controller (and so the GUI) sees:
    /// the window's DPI scale when windowed (the DPI virtualisation Cube.exe gets, see the
    /// module doc), 1 in fullscreen.
    fn gui_scale(&self) -> f64 {
        let Some(w) = &self.window else { return 1.0 };
        let s = w.inner_size();
        gui_layout((s.width, s.height), w.scale_factor(), self.fullscreen_resolution()).1
    }

    /// The resolution option when fullscreen (the argument of [`gui_layout`]).
    fn fullscreen_resolution(&self) -> Option<(i32, i32)> {
        let o = self.controller.as_ref()?.options;
        (o.fullscreen != 0).then_some((o.resolution_x, o.resolution_y))
    }

    /// `resetDevice` 0x004c8940 (WM_SIZE, WM_MOVE, and `frame` when the options changed):
    /// fullscreen (`0x0076b1d8`) presents at the resolution option (`0x0076b1dc`/`+0x76b1e0`)
    /// in an exclusive mode of that size when the monitor has one (borderless otherwise),
    /// windowed at the client size; the anti-aliasing option (`0x0076b1e4 * 2` samples, the
    /// presentation parameters' `MultiSampleType`) reconfigures the sink; then the back buffer
    /// is resized and the controller's `onResize` runs with the size (`GC+0x11c`/`+0x120`).
    fn reset_device(&mut self) {
        let (Some(w), Some(c)) = (self.window.as_ref(), self.controller.as_mut()) else { return };
        let o = c.options;
        let opts = [o.fullscreen, o.resolution_x, o.resolution_y, o.anti_aliasing];
        let changed_aa = self.device_options.is_none_or(|d| d[3] != opts[3]);
        let changed_mode = self.device_options.is_none_or(|d| d[0] != opts[0] || d[1] != opts[1] || d[2] != opts[2]);
        self.device_options = Some(opts);
        if changed_mode {
            if o.fullscreen != 0 {
                // The resolution is in GUI units (`logical_display_modes`).
                let mode = w.current_monitor().and_then(|m| {
                    let want = physical_mode_size((o.resolution_x, o.resolution_y), m.scale_factor());
                    m.video_modes().find(|v| (v.size().width, v.size().height) == want)
                });
                match mode {
                    Some(v) => w.set_fullscreen(Some(winit::window::Fullscreen::Exclusive(v))),
                    None => w.set_fullscreen(Some(winit::window::Fullscreen::Borderless(None))),
                }
            } else {
                w.set_fullscreen(None);
            }
        }
        // The back buffer is the window's physical size in both modes; fullscreen, the GUI is
        // laid out at the resolution option instead (`gui_layout`). A mode change resizes the
        // window asynchronously, and its `Resized` redoes this with the new size.
        let size = w.inner_size();
        let (width, height) = (size.width, size.height);
        if width == 0 || height == 0 {
            return;
        }
        if changed_aa && let Some(sink) = self.sink.as_mut() {
            sink.set_anti_aliasing(o.anti_aliasing);
        }
        if let Some(sink) = self.sink.as_mut() {
            sink.resize(width, height);
        }
        // The fullscreen flag may just have changed: the scale is re-read for the new mode.
        let fullscreen = (o.fullscreen != 0).then_some((o.resolution_x, o.resolution_y));
        let ((lw, lh), scale) = gui_layout((width, height), w.scale_factor(), fullscreen);
        c.set_back_buffer(width, height, scale);
        c.on_resize(lw, lh);
    }

    /// Mouse look: the cursor is held (the original recentres it with `SetCursorPos` each
    /// frame) and hidden while the cursor is not free; released otherwise.
    fn update_grab(&mut self, free: bool) {
        let Some(w) = &self.window else { return };
        let want = !free && self.focused;
        if want == self.grabbed {
            return;
        }
        if want {
            let ok = w.set_cursor_grab(CursorGrabMode::Locked).or_else(|_| w.set_cursor_grab(CursorGrabMode::Confined)).is_ok();
            w.set_cursor_visible(false);
            self.grabbed = ok;
            self.cursor_restored = false;
        } else {
            let _ = w.set_cursor_grab(CursorGrabMode::None);
            // The system cursor stays hidden over the window (`ShowCursor(FALSE)`); the GUI
            // draws `cursor.plx` (`GC+0x8008a8`) at the cursor while it is free.
            w.set_cursor_visible(false);
            self.grabbed = false;
        }
    }

    /// gilrs into the stick axes (`+0x124..+0x130`, XInput range).
    fn poll_gamepad(&mut self) {
        let Some(g) = &mut self.gilrs else { return };
        while g.next_event().is_some() {}
        let Some(c) = self.controller.as_mut() else { return };
        let mut ls = [0i32; 2];
        let mut rs = [0i32; 2];
        if let Some((_, pad)) = g.gamepads().next() {
            let ax = |a: gilrs::Axis| (pad.value(a) * 32767.0) as i32;
            ls = [ax(gilrs::Axis::LeftStickX), ax(gilrs::Axis::LeftStickY)];
            rs = [ax(gilrs::Axis::RightStickX), ax(gilrs::Axis::RightStickY)];
        }
        c.input.left_stick = ls;
        c.input.right_stick = rs;
    }

    /// One pass of the message loop after the messages: the device state, `frame`, the FPS
    /// limit.
    fn frame(&mut self, el: &ActiveEventLoop) {
        let t0 = Instant::now();
        let input_scope = profile::scope(Phase::Input);
        if let Some(q) = self.args.quit_after
            && self.started.elapsed().as_secs_f32() > q
            && let Some(c) = self.controller.as_mut()
        {
            c.quit = true;
        }
        let Some(c) = self.controller.as_mut() else { return };
        // `GC+0x1a0`: PostQuitMessage.
        if c.quit {
            el.exit();
            return;
        }
        // The scripted inputs whose time has come (`--key-at`, `--cursor-at`).
        let now_s = self.started.elapsed().as_secs_f32();
        let due: Vec<ScriptEvent> = self.args.script.iter().filter(|(t, _)| *t <= now_s).map(|(_, e)| *e).collect();
        let scale = self.gui_scale();
        let Some(c) = self.controller.as_mut() else { return };
        self.args.script.retain(|(t, _)| *t > now_s);
        for ev in due {
            match ev {
                ScriptEvent::Key(vk) => {
                    c.on_key_down(vk);
                    c.on_key_up(vk);
                }
                // Positions in physical client pixels, like `CursorMoved`.
                ScriptEvent::Cursor(x, y) => {
                    self.cursor = [f64::from(x) / scale, f64::from(y) / scale];
                    self.focused = true;
                }
                ScriptEvent::Click(x, y) => {
                    self.cursor = [f64::from(x) / scale, f64::from(y) / scale];
                    self.focused = true;
                    c.on_mouse_move(self.cursor[0] as f32, self.cursor[1] as f32);
                    c.input.buttons[MouseButton::Left as usize] = true;
                    c.on_mouse_down(MouseButton::Left);
                    c.input.buttons[MouseButton::Left as usize] = false;
                    c.on_mouse_up(MouseButton::Left);
                }
                ScriptEvent::Press(x, y) => {
                    self.cursor = [f64::from(x) / scale, f64::from(y) / scale];
                    self.focused = true;
                    c.on_mouse_move(self.cursor[0] as f32, self.cursor[1] as f32);
                    c.input.buttons[MouseButton::Left as usize] = true;
                    c.on_mouse_down(MouseButton::Left);
                }
                ScriptEvent::Release(x, y) => {
                    self.cursor = [f64::from(x) / scale, f64::from(y) / scale];
                    self.focused = true;
                    c.on_mouse_move(self.cursor[0] as f32, self.cursor[1] as f32);
                    c.input.buttons[MouseButton::Left as usize] = false;
                    c.on_mouse_up(MouseButton::Left);
                }
                ScriptEvent::Char(ch) => c.on_char(ch),
                ScriptEvent::Shot => {
                    if let Some(s) = self.sink.as_mut() {
                        s.request_capture();
                    }
                }
            }
        }
        let Some(c) = self.controller.as_mut() else { return };
        let free = c.is_cursor_free();
        // Port-only debug overlay: the cursor is released and mouse look suspended while it shows.
        #[cfg(feature = "debug-overlay")]
        let overlay = self.debug.visible();
        #[cfg(not(feature = "debug-overlay"))]
        let overlay = false;
        self.update_grab(free || overlay);
        self.poll_gamepad();
        let Some(c) = self.controller.as_mut() else { return };
        if let Some((x, y)) = slot6_motion(free, self.focused && !overlay, self.cursor, self.mouse_delta) {
            c.on_mouse_move(x, y);
        }
        self.mouse_delta = [0.0; 2];
        drop(input_scope);
        // `frame` 0x004c85f0.
        let dt = frame_dt(self.started, &mut self.last, Instant::now());
        c.update(dt);
        // `frame` 0x004c85f0 step 3: the options block against the device's copy.
        let o = c.options;
        let opts = [o.fullscreen, o.resolution_x, o.resolution_y, o.anti_aliasing];
        if self.device_options.is_some_and(|d| d != opts) {
            self.reset_device();
        }
        let Some(c) = self.controller.as_mut() else { return };
        if let Some(sink) = self.sink.as_mut() {
            // Port-only debug overlay (`debug_overlay.rs`): its UI and actions, its paint.
            #[cfg(feature = "debug-overlay")]
            if let Some(w) = &self.window {
                self.debug.frame(w, c, sink.as_mut(), scale);
            }
            c.render(sink.as_mut());
            if let Some((w, h, rgba)) = sink.take_capture() {
                self.shots += 1;
                let dir = std::env::var_os("CW_SHOT_DIR").map_or_else(|| std::path::PathBuf::from("."), std::path::PathBuf::from);
                let path = dir.join(format!("shot_{}.png", self.shots));
                match image::RgbaImage::from_raw(w, h, rgba).map(|i| i.save(&path)) {
                    Some(Ok(())) => eprintln!("cw-client: saved {}", path.display()),
                    other => eprintln!("cw-client: screenshot {}: {:?}", path.display(), other.map(|r| r.err())),
                }
            }
        }
        // The FPS limit: `Sleep(minTimeStep - frameTime)`. WinMain calls `timeBeginPeriod(1)`
        // so its `Sleep` has 1 ms granularity; `std::thread::sleep` on Windows 10 1803+ uses a
        // high-resolution waitable timer, which gives the same (measured: a 6 ms limit gives
        // 6.6..7.0 ms frames, as `timeGetTime`'s whole milliseconds do in the original).
        let spent = t0.elapsed().as_millis() as i32;
        let sleep = c.options.frame_sleep_ms(spent);
        if sleep > 0 {
            let _s = profile::scope(Phase::Sleep);
            std::thread::sleep(Duration::from_millis(u64::from(sleep)));
        }
        // The frame's time includes the window messages handled since the last one.
        let now = Instant::now();
        let total = self.frame_start.map_or(now - t0, |s| now - s);
        self.frame_start = Some(now);
        self.log.end_frame(total, dt);
    }
}

impl ApplicationHandler for App {
    /// `WinMain` 0x004c8ae0 steps 2..13: the window ("Cube", 800x600 then maximised), the
    /// device, the display modes, the options, the audio engine, the controller.
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes().with_title("Cube").with_inner_size(LogicalSize::new(800.0, 600.0)).with_maximized(true);
        let window = match el.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("cw-client: cannot create the window: {e}");
                el.exit();
                return;
            }
        };
        window.set_cursor_visible(false);
        let size = window.inner_size();
        let sink: Box<dyn FrameSink> = match ExecSink::new(Arc::clone(&window)) {
            Ok(s) => {
                if profile::enabled() {
                    let a = s.ctx.adapter.get_info();
                    eprintln!("cw-client: {} ({:?}), present mode {:?}", a.name, a.backend, s.ctx.config.present_mode);
                }
                Box::new(s)
            }
            Err(e) => {
                // The original shows "Could not initialize Direct3D" and exits; the port runs
                // without drawing.
                eprintln!("cw-client: could not initialise the GPU: {e}");
                Box::new(NullSink)
            }
        };
        // Step 5: the display modes (unique width/height pairs), in the logical units of the
        // DPI-virtualised process (`logical_display_modes`).
        let monitor = window.current_monitor();
        let monitor_scale = monitor.as_ref().map_or(window.scale_factor(), |m| m.scale_factor());
        let modes = monitor.as_ref().map_or_else(Vec::new, |m| {
            logical_display_modes(m.video_modes().map(|v| (v.size().width, v.size().height)), monitor_scale)
        });
        // Step 6: the screen size as the default resolution (`GetSystemMetrics(0/1)`, the
        // logical screen size in the virtualised process).
        let screen = monitor.as_ref().map_or((size.width, size.height), |m| (m.size().width, m.size().height));
        let screen = logical_size(screen.0, screen.1, monitor_scale);
        let screen = [screen.0, screen.1];
        let mut c = Controller::new(default_game_dir(), screen, modes, self.args.server_flag, self.args.server_address.clone());
        c.on_resize(size.width as i32, size.height as i32);
        if let Some((seed, name)) = self.args.start_world.clone() {
            // What the menu flow does on "Select": the screens hidden, the world started.
            let m = c.ui.m.clone();
            for n in [m.start_root, m.char_select_root, m.char_create_root, m.world_select_root, m.world_create_root, m.server_root] {
                c.ui.set_visible(n, false);
            }
            c.ui.set_visible(m.gui_root, true);
            // The world select's "Select" (0x00484170): the camera reset, then `startWorld`.
            c.apply_action(crate::ui::UiAction::ResetCamera { yaw: 180.0 });
            c.start_world(seed, &name);
        }
        self.controller = Some(c);
        self.sink = Some(sink);
        // Port-only debug overlay (`debug_overlay.rs`).
        #[cfg(feature = "debug-overlay")]
        self.debug.attach(&window);
        self.window = Some(window);
        // The WM_SIZE of `ShowWindow(SW_MAXIMIZE)` resets the device with the loaded options.
        self.reset_device();
        el.set_control_flow(ControlFlow::Poll);
    }

    /// `WndProc` 0x004c83f0.
    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        // Port-only debug overlay (`debug_overlay.rs`): F3 and the input egui takes.
        #[cfg(feature = "debug-overlay")]
        if let Some(w) = &self.window
            && self.debug.on_window_event(w, &event)
        {
            return;
        }
        let scale = self.gui_scale();
        let window_scale = self.window.as_ref().map_or(1.0, |w| w.scale_factor());
        let Some(c) = self.controller.as_mut() else { return };
        match event {
            // WM_CLOSE → DestroyWindow → WM_DESTROY → PostQuitMessage.
            WindowEvent::CloseRequested => {
                c.quit = true;
                el.exit();
            }
            // WM_SIZE → resetDevice 0x004c8940 → slot 4.
            WindowEvent::Resized(s) => {
                if s.width > 0 && s.height > 0 {
                    if let Some(sink) = self.sink.as_mut() {
                        sink.resize(s.width, s.height);
                    }
                    let fullscreen = (c.options.fullscreen != 0).then_some((c.options.resolution_x, c.options.resolution_y));
                    let ((lw, lh), scale) = gui_layout((s.width, s.height), window_scale, fullscreen);
                    c.set_back_buffer(s.width, s.height, scale);
                    c.on_resize(lw, lh);
                }
            }
            WindowEvent::Focused(f) => {
                self.focused = f;
                if !f {
                    c.input = crate::input::InputState::default();
                }
            }
            // `GetCursorPos` + `ScreenToClient` of a DPI-virtualised process: logical pixels.
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = [position.x / scale, position.y / scale];
            }
            // The original reads `GetCursorPos` every frame, so a cursor outside the window
            // gives client coordinates outside it and the engine's hit test finds nothing (the
            // hovered button gets its leave). winit reports no position outside the window
            // unless a button is held (a drag keeps its `CursorMoved`s).
            WindowEvent::CursorLeft { .. } => {
                self.cursor = cursor_after_leave(self.cursor, c.input.buttons);
            }
            // WM_*BUTTONDOWN/UP → slots 7/8; the DirectInput buttons.
            WindowEvent::MouseInput { state, button, .. } => {
                let b = match button {
                    winit::event::MouseButton::Left => MouseButton::Left,
                    winit::event::MouseButton::Right => MouseButton::Right,
                    winit::event::MouseButton::Middle => MouseButton::Middle,
                    _ => return,
                };
                let down = state == ElementState::Pressed;
                c.input.buttons[b as usize] = down;
                if down {
                    c.on_mouse_down(b);
                } else {
                    c.on_mouse_up(b);
                }
            }
            // WM_MOUSEWHEEL → slot 9 with the sign.
            WindowEvent::MouseWheel { delta, .. } => {
                let d = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32,
                };
                let s = if d > 0.0 {
                    1
                } else if d < 0.0 {
                    -1
                } else {
                    0
                };
                c.on_mouse_wheel(s);
            }
            // WM_KEYDOWN/WM_KEYUP → slots 2/3, WM_CHAR → slot 5; the DirectInput keys.
            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(code) = event.physical_key else { return };
                let down = event.state == ElementState::Pressed;
                if let Some(d) = dik_of(code) {
                    if down {
                        c.input.press(d);
                    } else {
                        c.input.release(d);
                    }
                }
                if let Some(v) = vk_of(code) {
                    if down {
                        c.on_key_down(v);
                    } else {
                        c.on_key_up(v);
                    }
                }
                if down && let Some(t) = &event.text {
                    for u in t.encode_utf16() {
                        c.on_char(u);
                    }
                }
            }
            WindowEvent::RedrawRequested => {}
            _ => {}
        }
    }

    /// `DIMOUSESTATE::lX/lY`: the raw motion.
    fn device_event(&mut self, _el: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = event {
            self.mouse_delta[0] += delta.0;
            self.mouse_delta[1] += delta.1;
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        self.frame(el);
    }

    /// The shutdown of `WinMain` (`timeEndPeriod`, the controller's deleting destructor, ...).
    fn exiting(&mut self, _el: &ActiveEventLoop) {
        self.log.finish();
        // The original's window is gone (WM_CLOSE → DestroyWindow) before the controller's
        // destructor saves the world; hidden here, so the saves below (seconds on a hard disk)
        // do not leave a window that stopped answering ("Not Responding").
        if let Some(w) = &self.window {
            w.set_visible(false);
        }
        if let Some(mut c) = self.controller.take() {
            c.shutdown();
        }
        self.sink = None;
    }
}

/// `WinMain`: builds the event loop and runs the application until it quits.
pub fn run(args: Args) -> Result<(), String> {
    let el = EventLoop::new().map_err(|e| e.to_string())?;
    el.set_control_flow(ControlFlow::Poll);
    let mut app = App::new(args);
    el.run_app(&mut app).map_err(|e| e.to_string())
}

/// `frame` 0x004c85f0: `dt = timeGetTime() - last`, `last` the previous frame's stamp (0 on
/// the first frame). The stamps are whole milliseconds since `started`, so no fraction of a
/// millisecond is lost between frames.
fn frame_dt(started: Instant, last: &mut Option<u64>, now: Instant) -> i32 {
    let ms = now.duration_since(started).as_millis() as u64;
    let dt = last.map_or(0, |l| ms.wrapping_sub(l) as i32);
    *last = Some(ms);
    dt
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `frame` 0x004c85f0 takes `dt` as the difference of two whole-millisecond
    /// `timeGetTime` stamps, so the frames' `dt`s add up to the elapsed time. Truncating each
    /// frame's own duration instead drops the fraction every frame (6.8 ms frames gave 6).
    #[test]
    fn frame_dt_loses_no_time_between_frames() {
        let start = Instant::now();
        let mut last = None;
        assert_eq!(frame_dt(start, &mut last, start), 0);
        let mut total = 0;
        for k in 1..=100u64 {
            total += frame_dt(start, &mut last, start + Duration::from_micros(6_800 * k));
        }
        assert_eq!(total, 680);
    }

    #[test]
    fn leaving_the_window_takes_the_cursor_off_the_gui() {
        assert_eq!(cursor_after_leave([30.0, 20.0], [false; 3]), [-1.0, -1.0]);
        assert_eq!(cursor_after_leave([30.0, 20.0], [true, false, false]), [30.0, 20.0]);
        // The engine then un-hovers the button the cursor left the window over.
        let mut g = cw_ui::widget::Gui::new();
        let root = g.add_plain_node(None, "root");
        g.root = Some(root);
        let b = g.add_plain_node(Some(root), "b");
        g.nodes[b].shape = Some(Box::new(Square));
        g.inject_mouse_move(5.0, 5.0, None);
        assert_eq!(g.hovered, Some(b));
        let c = cursor_after_leave([5.0, 5.0], [false; 3]);
        g.inject_mouse_move(c[0] as f32, c[1] as f32, None);
        assert_eq!(g.hovered, None);
    }

    #[derive(Debug)]
    struct Square;
    impl cw_ui::widget::HitShape for Square {
        fn contains_point(&self, p: glam::Vec2) -> bool {
            (0.0..10.0).contains(&p.x) && (0.0..10.0).contains(&p.y)
        }
        fn bounds(&self) -> (glam::Vec2, glam::Vec2) {
            (glam::Vec2::ZERO, glam::Vec2::splat(10.0))
        }
    }

    #[test]
    fn free_cursor_follows_the_mouse_without_the_focus() {
        // macOS can leave the window unfocused (a system prompt took the key status): the
        // menu cursor must still track `CursorMoved`.
        assert_eq!(slot6_motion(true, false, [10.0, 20.0], [0.0; 2]), Some((10.0, 20.0)));
        assert_eq!(slot6_motion(true, true, [10.0, 20.0], [3.0, 4.0]), Some((10.0, 20.0)));
        // Mouse look only with the focus (the grab is released without it).
        assert_eq!(slot6_motion(false, true, [10.0, 20.0], [3.0, 4.0]), Some((3.0, 4.0)));
        assert_eq!(slot6_motion(false, false, [10.0, 20.0], [3.0, 4.0]), None);
    }

    #[test]
    fn command_line() {
        let a = parse_args(&["cube".into(), "server".into()]);
        assert!(a.server_flag && a.server_address.is_empty());
        let a = parse_args(&["cube".into(), "10.0.0.1".into()]);
        assert!(!a.server_flag && a.server_address == "10.0.0.1");
        let a = parse_args(&["cube".into(), "--quit-after".into(), "3".into()]);
        assert_eq!(a.quit_after, Some(3.0));
    }

    /// Windowed, the controller gets the logical client size (the DPI virtualisation of the
    /// module doc) and the GUI is drawn at `scale_factor` device pixels per unit.
    #[test]
    fn windowed_gui_is_the_logical_client_size() {
        assert_eq!(gui_layout((3456, 1956), 2.0, None), ((1728, 978), 2.0));
        assert_eq!(gui_layout((1920, 1080), 1.0, None), ((1920, 1080), 1.0));
        assert_eq!(gui_layout((1, 1), 2.0, None), ((1, 1), 2.0));
    }

    /// `resetDevice` 0x004c8940: fullscreen, the back buffer and `GC+0x11c/+0x120` are the
    /// resolution option, so the GUI (laid out in back-buffer pixels) covers the fraction of
    /// the screen that resolution implies. The port's back buffer is the window's physical
    /// size: the GUI is laid out at the resolution and drawn scaled up to it.
    #[test]
    fn fullscreen_gui_is_laid_out_at_the_resolution_option() {
        // Borderless on a Retina panel (3456x2234 pixels, scale 2) at its logical size: the
        // same GUI size as a maximised window, not the 3456x2234 pixels at scale 1.
        assert_eq!(gui_layout((3456, 2234), 2.0, Some((1728, 1117))), ((1728, 1117), 2.0));
        // A lower resolution: bigger GUI, as the monitor's upscale of the original's mode.
        assert_eq!(gui_layout((3456, 2234), 2.0, Some((1152, 744))).1, 3.0);
        // 1280x720 on a 1920x1080 monitor at scale 1.
        assert_eq!(gui_layout((1920, 1080), 1.0, Some((1280, 720))), ((1280, 720), 1.5));
        // An exclusive mode of the resolution itself: scale 1.
        assert_eq!(gui_layout((1280, 720), 1.0, Some((1280, 720))), ((1280, 720), 1.0));
        // Another aspect ratio: the scale fits the resolution in and the GUI fills the screen
        // (at least the resolution on both axes).
        let ((w, h), s) = gui_layout((3456, 2234), 2.0, Some((1280, 720)));
        assert_eq!(s, 2.7);
        assert_eq!((w, h), (1280, 827));
        // No resolution (0x0): the windowed layout rather than a division by zero.
        assert_eq!(gui_layout((1920, 1080), 2.0, Some((0, 0))), ((960, 540), 2.0));
    }

    /// `WinMain` 0x004c8ae0 steps 5..6 in the virtualised process: the modes and the default
    /// resolution (`GetSystemMetrics(0/1)`) in the logical units the windowed GUI uses, so the
    /// default fullscreen resolution gives the windowed GUI size.
    #[test]
    fn display_modes_are_in_gui_units() {
        let modes = [(1920, 1200), (1920, 1200), (2624, 1696), (3456, 2234), (3456, 2234)];
        assert_eq!(logical_display_modes(modes, 2.0), vec![(960, 600), (1312, 848), (1728, 1117)]);
        assert_eq!(logical_display_modes([(1280, 720), (1920, 1080)], 1.0), vec![(1280, 720), (1920, 1080)]);
        assert_eq!(physical_mode_size((1728, 1117), 2.0), (3456, 2234));
        assert_eq!(physical_mode_size((1280, 720), 1.5), (1920, 1080));
    }

    #[test]
    fn key_codes() {
        assert_eq!(dik_of(KeyCode::KeyW), Some(0x11));
        assert_eq!(vk_of(KeyCode::KeyB), Some(0x42));
        assert_eq!(vk_of(KeyCode::F1), Some(0x70));
        assert_eq!(vk_of(KeyCode::Enter), Some(0x0d));
    }
}
