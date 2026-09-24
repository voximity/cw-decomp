//! `cw-client`: the Cube World Alpha client (Cube.exe). `main` is `WinMain` 0x004c8ae0
//! (`app::run`); the game logic is `controller::Controller` (`cube::GameController`).

mod app;
mod assets;
mod audio;
mod controller;
/// Port-only: the F3 debug overlay (egui), feature `debug-overlay`.
#[cfg(feature = "debug-overlay")]
mod debug_overlay;
mod input;
mod interact;
mod landscape;
mod map_screen;
mod names;
mod net;
mod options;
mod particles;
mod persist;
mod player;
mod profile;
mod prompts;
mod received;
mod save_writer;
mod scene;
mod singleplayer;
mod threads;
mod ui;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let a = app::parse_args(&args);
    if let Err(e) = app::run(a) {
        eprintln!("cw-client: {e}");
        std::process::exit(1);
    }
}
