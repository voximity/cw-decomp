//! `cwtool`: inspect and extract Cube World Alpha data.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use cw_formats::{AssetDb, CubModel, SaveDb};
use cw_world::save::{BlobReader, MissionState, MonsterState, ZoneBlob};

#[derive(Parser)]
#[command(
    name = "cwtool",
    version,
    about = "Inspect and extract Cube World Alpha data"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Asset databases (data1.db to data4.db)
    #[command(subcommand)]
    Db(DbCommand),
    /// .cub voxel models
    #[command(subcommand)]
    Cub(CubCommand),
    /// World save databases (Save/world_<name>.db)
    #[command(subcommand)]
    Save(SaveCommand),
    /// .plx Plasma graphics documents (gui.plx, start.plx, ...)
    #[command(subcommand)]
    Plx(PlxCommand),
}

#[derive(Subcommand)]
enum PlxCommand {
    /// Print the chunk tree with decrypted names, sizes and value previews
    Dump { file: PathBuf },
}

#[derive(Subcommand)]
enum SaveCommand {
    /// Print every key, one per line
    List { db: PathBuf },
    /// Write one blob's bytes to stdout (save blobs are not obfuscated)
    Cat { db: PathBuf, key: String },
    /// Print one blob decoded: time, zone<x>_<y>, mission<a>_<b> or monster<a>_<b>
    Show { db: PathBuf, key: String },
}

#[derive(Subcommand)]
enum DbCommand {
    /// Print every key, one per line
    List { db: PathBuf },
    /// Write one decoded blob to stdout
    Cat { db: PathBuf, key: String },
    /// Write every decoded blob into a directory, named by key
    Extract {
        db: PathBuf,
        /// Output directory, created if missing
        #[arg(long)]
        out: PathBuf,
    },
}

#[derive(Subcommand)]
enum CubCommand {
    /// Print dimensions and voxel counts of a decoded .cub file
    Info { file: PathBuf },
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    match cli.command {
        Command::Db(DbCommand::List { db }) => {
            let db = open(&db)?;
            let mut out = std::io::stdout().lock();
            for key in db.keys()? {
                writeln!(out, "{key}")?;
            }
        }
        Command::Db(DbCommand::Cat { db, key }) => {
            let bytes = open(&db)?.get(&key)?;
            std::io::stdout().lock().write_all(&bytes)?;
        }
        Command::Db(DbCommand::Extract { db, out }) => {
            let db = open(&db)?;
            std::fs::create_dir_all(&out)?;
            let keys = db.keys()?;
            for key in &keys {
                if key.contains('/') || key.contains('\\') || key.contains("..") {
                    return Err(
                        format!("refusing to extract key with path characters: {key:?}").into(),
                    );
                }
                std::fs::write(out.join(key), db.get(key)?)?;
            }
            println!("extracted {} blobs to {}", keys.len(), out.display());
        }
        Command::Save(SaveCommand::List { db }) => {
            let db = open_save(&db)?;
            let mut out = std::io::stdout().lock();
            for key in db.keys()? {
                writeln!(out, "{key}")?;
            }
        }
        Command::Save(SaveCommand::Cat { db, key }) => {
            let bytes = open_save(&db)?.get(&key)?.ok_or_else(|| format!("blob {key:?} not found"))?;
            std::io::stdout().lock().write_all(&bytes)?;
        }
        Command::Save(SaveCommand::Show { db, key }) => {
            let bytes = open_save(&db)?.get(&key)?.ok_or_else(|| format!("blob {key:?} not found"))?;
            print!("{}", show_save_blob(&key, &bytes));
        }
        Command::Cub(CubCommand::Info { file }) => {
            let bytes = std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            let model = CubModel::parse(&bytes)?;
            let [x, y, z] = model.size;
            let filled = model.voxels.iter().filter(|v| **v != [0, 0, 0]).count();
            println!("size: {x} x {y} x {z}");
            println!("voxels: {}", model.voxels.len());
            println!("filled: {filled}");
        }
        Command::Plx(PlxCommand::Dump { file }) => {
            let bytes = std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            let doc = cw_formats::plx::parse(&bytes).map_err(|e| format!("{}: {e}", file.display()))?;
            print!("{}", doc.dump());
        }
    }
    Ok(())
}

fn open(path: &PathBuf) -> Result<AssetDb, Box<dyn std::error::Error>> {
    if !path.is_file() {
        return Err(format!("{}: no such file", path.display()).into());
    }
    Ok(AssetDb::open(path).map_err(|e| format!("{}: {e}", path.display()))?)
}

fn open_save(path: &PathBuf) -> Result<SaveDb, Box<dyn std::error::Error>> {
    if !path.is_file() {
        return Err(format!("{}: no such file", path.display()).into());
    }
    Ok(SaveDb::open(path).map_err(|e| format!("{}: {e}", path.display()))?)
}

/// A readable rendering of one save blob, chosen by its key.
fn show_save_blob(key: &str, bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let blocks = |v: i64| v as f64 / 65536.0;
    let mut out = String::new();
    if key == "time" {
        let mut r = BlobReader::new(bytes);
        let day = r.i32().unwrap_or(0);
        let ms = r.i32().unwrap_or(0);
        let _ = writeln!(out, "day {day}, time of day {ms} ms ({:02}:{:02}:{:02})", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60);
    } else if key.starts_with("zone") {
        let z = ZoneBlob::parse(bytes, None, None);
        let _ = writeln!(out, "version {}", z.version);
        let _ = writeln!(out, "{} ground items", z.items.len());
        for it in &z.items {
            let _ = writeln!(
                out,
                "  item type {} sub {} level {} material {} rarity {} at ({:.2}, {:.2}, {:.2}) rotation {} f134 {} b138 {} f13c {} f140 {} day {}",
                it.item.item_type, it.item.sub_type, it.item.level, it.item.material, it.item.rarity, blocks(it.x), blocks(it.y), blocks(it.z), it.rotation, it.f134, it.b138, it.f13c, it.f140, it.f144
            );
        }
        let _ = writeln!(out, "{} modified blocks", z.blocks.len());
        for m in &z.blocks {
            let _ = writeln!(out, "  ({}, {}, {}) rgb ({}, {}, {}) type {:#04x} day {}", m.x, m.y, m.z, m.block[0], m.block[1], m.block[2], m.block[3], m.time);
        }
        match &z.spawn_words {
            Some(w) => {
                let _ = writeln!(out, "{} spawns", w.len());
                for (a, b) in w {
                    let _ = writeln!(out, "  +0x38 {} +0x3c {}", a.map_or("?".to_string(), |v| v.to_string()), b.map_or("?".to_string(), |v| v.to_string()));
                }
            }
            None => {
                let _ = writeln!(out, "spawn section skipped");
            }
        }
        match &z.static_bytes {
            Some(b) => {
                let _ = writeln!(out, "{} statics", b.len());
                let _ = writeln!(out, "  +0x30 {}", b.iter().map(|v| v.map_or("?".to_string(), |v| v.to_string())).collect::<Vec<_>>().join(" "));
            }
            None => {
                let _ = writeln!(out, "static section skipped or empty");
            }
        }
    } else if key.starts_with("mission") {
        let mut m = MissionState::default();
        m.read_blob(bytes);
        let _ = writeln!(out, "{m:?}");
    } else if key.starts_with("monster") {
        let mut m = MonsterState::default();
        m.read_blob(bytes);
        let (zx, zy) = m.zone_xy();
        let _ = writeln!(out, "kind {} level {} b5c {} zone ({zx}, {zy})", m.kind, m.level, m.b5c);
    } else {
        let _ = writeln!(out, "unknown key: {} bytes", bytes.len());
    }
    out
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
