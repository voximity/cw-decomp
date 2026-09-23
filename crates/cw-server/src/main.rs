//! `cw-server`: the dedicated server, following Server.exe's threads and packet order
//! (`analysis/notes/protocol.md`, section 3). Of the world tick, the clock, the mission
//! rolls of a region a player enters, mission discovery and the creatures the zones around
//! players spawn are ported; creatures do not move or fight yet, and hits, shoots and
//! passives are relayed to every client unchanged.
//!
//! A thin binary over the library (`lib.rs`): it parses the command line and starts the
//! threads of `main` 0x00549c50.

use cw_server::{conn, server, worldgen};

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;

#[derive(Parser)]
#[command(name = "cw-server", version, about = "Cube World Alpha dedicated server")]
struct Cli {
    /// World seed (`server.cfg` of the original holds only this number).
    #[arg(long, default_value_t = 26879)]
    seed: i32,
    /// Directory with data1.db (models) and where `Save/` is created (`CW_GAME_DIR` when
    /// set, as the client does).
    #[arg(long, env = "CW_GAME_DIR", default_value = "game")]
    game_dir: PathBuf,
    /// TCP port to listen on.
    #[arg(long, default_value_t = 12345)]
    port: u16,
    /// Do not open or create the save database.
    #[arg(long)]
    no_save: bool,
    /// Log every client packet (entity updates once per hundred) and every creature spawned.
    #[arg(long)]
    verbose: bool,
}

fn main() {
    let cli = Cli::parse();
    let server = match server::Server::new(cli.seed, &cli.game_dir, !cli.no_save) {
        Ok(mut s) => {
            s.verbose = cli.verbose;
            Arc::new(s)
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    println!("Starting server...");
    let listener = match std::net::TcpListener::bind(("0.0.0.0", cli.port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("error: cannot listen on port {}: {e}", cli.port);
            std::process::exit(1);
        }
    };
    println!("Server started. Enter Q to quit.");
    let tick = {
        let s = Arc::clone(&server);
        std::thread::Builder::new().name("tick".into()).spawn(move || server::tick_loop(s)).expect("tick thread")
    };
    let generation = {
        let s = Arc::clone(&server);
        std::thread::Builder::new().name("generation".into()).spawn(move || worldgen::generation_loop(s)).expect("generation thread")
    };
    {
        let s = Arc::clone(&server);
        std::thread::Builder::new().name("console".into()).spawn(move || console_quit_loop(s)).expect("console thread");
    }
    {
        let s = Arc::clone(&server);
        std::thread::Builder::new().name("accept".into()).spawn(move || conn::accept_loop(s, listener)).expect("accept thread");
    }
    // `main` 0x0054a060..0x0054a0e4: once the loop flag clears, the tick stops, the generation
    // thread is stopped and waited for ("stopping generation thread..."), and deleting the
    // world saves every loaded zone and every region's cells (`World::~World` 0x004cd940).
    let _ = tick.join();
    println!("stopping generation thread...");
    let _ = generation.join();
    server.world.lock().unwrap_or_else(|e| e.into_inner()).save_all();
    std::process::exit(0);
}

/// `lambda_consoleQuitThread` 0x00549b50: every second, one character from the console
/// (`cin >> c`, skipping white space); a lowercase `q` clears the main loop's flag. At the end
/// of input `cin` fails and leaves the character alone, so the loop never ends.
fn console_quit_loop(server: Arc<server::Server>) {
    use std::io::Read;
    use std::sync::atomic::Ordering;
    let mut stdin = std::io::stdin().lock();
    let mut c = 0u8;
    loop {
        std::thread::sleep(std::time::Duration::from_millis(1000));
        let mut b = [0u8; 1];
        while let Ok(1) = stdin.read(&mut b) {
            if !b[0].is_ascii_whitespace() {
                c = b[0];
                break;
            }
        }
        if c == b'q' {
            break;
        }
    }
    server.running.store(false, Ordering::Relaxed);
}
