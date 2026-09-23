//! Sound playback: `GameController::playSound` 0x00484350 over `cube::XAudio2Engine`
//! (Cube.exe; vtable 0x0071e23c), reimplemented on `kira`.
//!
//! What is kept from the original (Tier B, the cue the game computes):
//!
//! - the 101 sound ids of `playSound`'s switch and the `.wav` blob each loads from `data2.db`
//!   ([`SOUND_NAMES`]);
//! - the listener test and falloff: nothing plays beyond 100 blocks, the gain is
//!   `(1 - dist / 100) * volume`, and the engine gets `soundVolume * gain * 0.01`;
//! - the pan `clamp(-0.1 * clip.x / clip.w, -1, 1)` from the view-projection matrix at
//!   `GameController+0x22c`. XAudio2Engine::playSound (slot 6, 0x00623610) takes the pan and
//!   never uses it, so every sound of the original plays centred: [`AudioEngine::centred_audio`]
//!   (false by default, see docs/TODO.md "Client decisions") set reproduces that, clear applies
//!   the computed pan;
//! - the pitch as a frequency ratio (`IXAudio2SourceVoice::SetFrequencyRatio`, voices created
//!   with a maximum ratio of 2.0), which is a playback-rate change of speed and pitch together;
//! - `loadSound` (slot 7, 0x00623a60): blobs decoded with `decodeBlob`, cached by name, handles
//!   numbered from 1 in load order; a blob that is not RIFF/WAVE gets no handle and every
//!   later request retries it.
//!
//! The device, the mixer and the voice management are kira's (Tier C). `XAudio2Engine`'s
//! per-frame `reapFinishedVoices` (slot 0) has no counterpart: kira frees finished sounds.
//! There is no music: `cube::Music` exists but nothing plays it and no `.ogg` ships.

#![allow(dead_code)]
// The comparisons keep the original's NaN behaviour.
#![allow(clippy::neg_cmp_op_on_partial_ord)]

use std::collections::HashMap;

use cw_formats::AssetDb;

/// `playSound` 0x00484350's switch: sound id → `.wav` blob of `data2.db`. Ids 0x14, 0x15,
/// 0x38, 0x3e/0x3f and 0x52 reuse the blob of another id; any id above 100 falls to the
/// switch's default. `cry.wav` (0x1c), `fireball.wav` (0x26) and `bird3.wav` (0x60) are not
/// in the shipped `data2.db` (it has `Fireball.wav`; the lookup is case-sensitive), so those
/// three ids are silent in the original too.
pub const SOUND_NAMES: [&str; 101] = [
    "hit.wav",                  // 0x00
    "blade1.wav",               // 0x01
    "blade2.wav",               // 0x02
    "long-blade1.wav",          // 0x03
    "long-blade2.wav",          // 0x04
    "hit1.wav",                 // 0x05
    "hit2.wav",                 // 0x06
    "punch1.wav",               // 0x07
    "punch2.wav",               // 0x08
    "hit-arrow.wav",            // 0x09
    "hit-arrow-critical.wav",   // 0x0a
    "smash1.wav",               // 0x0b
    "slam-ground.wav",          // 0x0c
    "smash-hit2.wav",           // 0x0d
    "smash-jump.wav",           // 0x0e
    "swing.wav",                // 0x0f
    "shield-swing.wav",         // 0x10
    "swing-slow.wav",           // 0x11
    "swing-slow2.wav",          // 0x12
    "arrow-destroy.wav",        // 0x13
    "blade1.wav",               // 0x14
    "punch2.wav",               // 0x15
    "salvo2.wav",               // 0x16
    "sword-hit03.wav",          // 0x17
    "block.wav",                // 0x18
    "shield-slam.wav",          // 0x19
    "roll.wav",                 // 0x1a
    "destroy2.wav",             // 0x1b
    "cry.wav",                  // 0x1c
    "levelup2.wav",             // 0x1d
    "missioncomplete.wav",      // 0x1e
    "water-splash01.wav",       // 0x1f
    "step2.wav",                // 0x20
    "step-water.wav",           // 0x21
    "step-water2.wav",          // 0x22
    "step-water3.wav",          // 0x23
    "channel2.wav",             // 0x24
    "channel-hit.wav",          // 0x25
    "fireball.wav",             // 0x26
    "fire-hit.wav",             // 0x27
    "magic02.wav",              // 0x28
    "watersplash.wav",          // 0x29
    "watersplash-hit.wav",      // 0x2a
    "lich-scream.wav",          // 0x2b
    "drink2.wav",               // 0x2c
    "pickup.wav",               // 0x2d
    "disenchant2.wav",          // 0x2e
    "upgrade2.wav",             // 0x2f
    "swirl.wav",                // 0x30
    "human-voice01.wav",        // 0x31
    "human-voice02.wav",        // 0x32
    "gate.wav",                 // 0x33
    "spike-trap.wav",           // 0x34
    "fire-trap.wav",            // 0x35
    "lever.wav",                // 0x36
    "charge2.wav",              // 0x37
    "magic02.wav",              // 0x38
    "drop.wav",                 // 0x39
    "drop-coin.wav",            // 0x3a
    "drop-item.wav",            // 0x3b
    "male-groan.wav",           // 0x3c
    "female-groan.wav",         // 0x3d
    "male-groan.wav",           // 0x3e
    "female-groan.wav",         // 0x3f
    "goblin-male-groan.wav",    // 0x40
    "goblin-female-groan.wav",  // 0x41
    "lizard-male-groan.wav",    // 0x42
    "lizard-female-groan.wav",  // 0x43
    "dwarf-male-groan.wav",     // 0x44
    "dwarf-female-groan.wav",   // 0x45
    "orc-male-groan.wav",       // 0x46
    "orc-female-groan.wav",     // 0x47
    "undead-male-groan.wav",    // 0x48
    "undead-female-groan.wav",  // 0x49
    "frogman-male-groan.wav",   // 0x4a
    "frogman-female-groan.wav", // 0x4b
    "monster-groan.wav",        // 0x4c
    "troll-groan.wav",          // 0x4d
    "mole-groan.wav",           // 0x4e
    "slime-groan.wav",          // 0x4f
    "zombie-groan.wav",         // 0x50
    "Explosion.wav",            // 0x51
    "punch2.wav",               // 0x52
    "menu-open2.wav",           // 0x53
    "menu-close2.wav",          // 0x54
    "menu-select.wav",          // 0x55
    "menu-tab.wav",             // 0x56
    "menu-grab-item.wav",       // 0x57
    "menu-drop-item.wav",       // 0x58
    "craft.wav",                // 0x59
    "craft-proc.wav",           // 0x5a
    "absorb.wav",               // 0x5b
    "manashield.wav",           // 0x5c
    "bulwark.wav",              // 0x5d
    "bird1.wav",                // 0x5e
    "bird2.wav",                // 0x5f
    "bird3.wav",                // 0x60
    "cricket1.wav",             // 0x61
    "cricket2.wav",             // 0x62
    "owl1.wav",                 // 0x63
    "owl2.wav",                 // 0x64
];

/// Named ids used by the client code itself (`analysis/notes/client-classes.md`, "Where sounds
/// are played from").
pub mod id {
    pub const DESTROY: u32 = 0x1b;
    pub const EXPLOSION: u32 = 0x51;
    pub const PUNCH_SHAKE: u32 = 0x52;
    pub const MENU_OPEN: u32 = 0x53;
    pub const MENU_CLOSE: u32 = 0x54;
    pub const MENU_SELECT: u32 = 0x55;
    pub const CRICKET: u32 = 0x61;
}

/// The blob name of a sound id, `None` for the switch's default (ids above 0x64).
pub fn sound_name(id: u32) -> Option<&'static str> {
    SOUND_NAMES.get(id as usize).copied()
}

/// What `playSound` reads of the controller: the listener position (`GameController+0x140`,
/// fixed point 16.16), the view-projection matrix (`+0x22c`, sixteen floats in D3D row-major
/// order, row vectors: `clip = [x y z 1] * M`) and `soundVolume` (`+0x184`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Listener {
    pub pos: [i64; 3],
    pub view_proj: [f32; 16],
    pub sound_volume: i32,
}

/// The numbers `playSound` hands the engine for one sound.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cue {
    /// `soundVolume * gain * 0.01`, a linear amplitude (`IXAudio2Voice::SetVolume`).
    pub volume: f32,
    /// `clamp(-0.1 * clip.x / clip.w, -1, 1)`; the original drops it.
    pub pan: f32,
    /// The frequency ratio, as passed.
    pub pitch: f32,
}

/// `GameController::playSound` 0x00484350 without the engine call: `None` when the sound is
/// more than 100 blocks from the listener (then nothing, not even `loadSound`, happens).
pub fn compute_cue(listener: &Listener, pos: [i64; 3], volume: f32, pitch: f32) -> Option<Cue> {
    const INV: f32 = 1.525_878_9e-5; // 1 / 65536
    // 0x00484390..: the 64-bit difference, `fild` into f32 (one rounding), times 1/65536.
    let d = |i: usize| (pos[i].wrapping_sub(listener.pos[i]) as f32) * INV;
    let (dx, dy, dz) = (d(0), d(1), d(2));
    // `comiss`: skip when `10000 < (dy*dy + dx*dx) + dz*dz`; NaN falls through.
    let sq = (dy * dy + dx * dx) + dz * dz;
    if 10000.0f32 < sq {
        return None;
    }
    // The distance is recomputed the same way, `sqrt` in double, then used as a float.
    let dist = f64::from(sq).sqrt() as f32;
    let gain = (1.0f32 - dist / 100.0f32) * volume;
    // 0x00485d.. after the switch: the absolute position (not the offset) through the matrix.
    let m = &listener.view_proj;
    // x and y are scaled first; z stays raw and the 1/65536 applies after the matrix
    // element (`m[z] * z_raw * 1.5258789e-05`), as the compiled code orders it.
    let px = (pos[0] as f32) * INV;
    let py = (pos[1] as f32) * INV;
    let pz_raw = pos[2] as f32;
    let w = m[3] * px + m[7] * py + m[11] * pz_raw * INV + m[15];
    let x = m[4] * py + m[0] * px + m[8] * pz_raw * INV + m[12];
    let raw = (1.0f32 / w) * x * -0.1f32;
    // `if (!(-1 <= v)) v = -1; else if (1 < v) v = 1;` (NaN becomes -1).
    let pan = if !(-1.0f32 <= raw) {
        -1.0
    } else if 1.0f32 < raw {
        1.0
    } else {
        raw
    };
    let volume = (listener.sound_volume as f32) * gain * 0.01f32;
    Some(Cue { volume, pan, pitch })
}

/// A server sound record (`cw_net::packet::Sound`, 0x18 bytes, ServerUpdate section 4) as
/// `GameController::update` reads it (0x004955f0..0x00495642): `f32 x, y, z` in blocks, the
/// sound id at `+0xc`, the pitch at `+0x10`, the volume at `+0x14`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SoundRecord {
    /// `vec3i64` from the block position: `__ftol2(v * 65536.0f)` per axis (0x0042c460).
    pub pos: [i64; 3],
    pub id: u32,
    pub pitch: f32,
    pub volume: f32,
}

impl SoundRecord {
    pub fn from_net(s: &cw_net::packet::Sound) -> SoundRecord {
        let b = &s.0;
        let f = |o: usize| f32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        let fix = |v: f32| (v * 65536.0f32) as i64;
        SoundRecord {
            pos: [fix(f(0)), fix(f(4)), fix(f(8))],
            id: u32::from_le_bytes(b[0xc..0x10].try_into().unwrap()),
            pitch: f(0x10),
            volume: f(0x14),
        }
    }

    /// After playing, ids 0x51 (Explosion) and 0x52 set `GameController+0x1e8` to 1.0
    /// (0x00495635..0x00495642), the camera shake.
    pub fn shakes_camera(&self) -> bool {
        self.id == id::EXPLOSION || self.id == id::PUNCH_SHAKE
    }
}

/// XAudio2 limits of the voices `playSound` creates: `XAUDIO2_MIN_FREQ_RATIO` and the
/// `MaxFrequencyRatio` of 2.0 passed to `CreateSourceVoice`.
pub const MIN_FREQ_RATIO: f32 = 1.0 / 1024.0;
pub const MAX_FREQ_RATIO: f32 = 2.0;

/// `cube::XAudio2Engine` on kira: the sound cache of `loadSound` and the playback of
/// `playSound` (slot 6).
pub struct AudioEngine {
    manager: Option<kira::AudioManager>,
    /// `data2.db` (`XAudio2Engine+0x2c`), opened by the constructor 0x00622da0.
    db: Option<AssetDb>,
    /// `loadSound`'s name cache (`+0x1c`): name → handle, handles from 1 (`+0x30`).
    handles: HashMap<String, u32>,
    /// Handle - 1 → decoded sound (`+0x30`'s map of `XAudio2Engine::Sound`).
    sounds: Vec<kira::sound::static_sound::StaticSoundData>,
    /// Play every sound centred, as the original does (it computes a pan and drops it).
    pub centred_audio: bool,
}

impl AudioEngine {
    /// Opens the audio device and `data2.db`. Either may be missing: without a device nothing
    /// is heard, without the database nothing loads. (The original shows "Could not initialize
    /// XAudio2" and does not start the game when the device fails.)
    pub fn new(db: Option<AssetDb>) -> AudioEngine {
        let manager = kira::AudioManager::<kira::DefaultBackend>::new(kira::AudioManagerSettings::default()).ok();
        AudioEngine { manager, db, handles: HashMap::new(), sounds: Vec::new(), centred_audio: false }
    }

    /// An engine with no device, for tests and headless runs.
    pub fn silent(db: Option<AssetDb>) -> AudioEngine {
        AudioEngine { manager: None, db, handles: HashMap::new(), sounds: Vec::new(), centred_audio: false }
    }

    pub fn has_device(&self) -> bool {
        self.manager.is_some()
    }

    /// `XAudio2Engine::loadSound` 0x00623a60: the handle of `name`, loading it on first use
    /// (`getBlobVec` 0x004498d0 + `decodeBlob` 0x004496a0, then the RIFF chunks). 0 when the
    /// blob is missing or not RIFF/WAVE; such a name is looked up again next time.
    pub fn load_sound(&mut self, name: &str) -> u32 {
        if let Some(&h) = self.handles.get(name) {
            return h;
        }
        let Some(db) = &self.db else { return 0 };
        // `AssetDb::get` is getBlobVec followed by decodeBlob. The original's lookup is
        // case-sensitive and three names of the switch (`cry.wav`, `fireball.wav`,
        // `bird3.wav`) miss because of it; the port retries case-insensitively (the database
        // holds `Fireball.wav`), a deliberate fix: see docs/TODO.md "Client decisions".
        let bytes = match db.get(name) {
            Ok(b) => b,
            Err(_) => {
                let lower = name.to_ascii_lowercase();
                let Ok(keys) = db.keys() else { return 0 };
                let Some(k) = keys.into_iter().find(|k| k.to_ascii_lowercase() == lower) else { return 0 };
                let Ok(b) = db.get(&k) else { return 0 };
                b
            }
        };
        if !is_riff_wave(&bytes) {
            return 0;
        }
        let Ok(data) = kira::sound::static_sound::StaticSoundData::from_cursor(std::io::Cursor::new(bytes)) else {
            return 0;
        };
        self.sounds.push(data);
        let h = self.sounds.len() as u32;
        self.handles.insert(name.to_string(), h);
        h
    }

    /// `XAudio2Engine::playSound` (slot 6, 0x00623610): nothing unless `0 < volume` and the
    /// handle names a loaded sound; otherwise a new voice at `volume` and the frequency ratio
    /// `pitch`. The pan is applied only when [`AudioEngine::centred_audio`] is off.
    pub fn play_handle(&mut self, handle: u32, cue: &Cue) {
        if !(0.0f32 < cue.volume) {
            return;
        }
        if handle == 0 {
            return;
        }
        let Some(data) = self.sounds.get(handle as usize - 1) else { return };
        let Some(manager) = &mut self.manager else { return };
        let db = 20.0 * cue.volume.log10();
        let rate = f64::from(cue.pitch.clamp(MIN_FREQ_RATIO, MAX_FREQ_RATIO));
        let pan = if self.centred_audio { 0.0 } else { cue.pan };
        let sound = data.volume(db).playback_rate(rate).panning(pan);
        // A failed play (too many sounds) is dropped, like a failed CreateSourceVoice.
        let _ = manager.play(sound);
    }

    /// `GameController::playSound` 0x00484350: sound `id` at `pos` with `volume` and `pitch`.
    /// An id outside the switch loads nothing and plays handle 0, which is never a sound.
    pub fn play_sound(&mut self, listener: &Listener, id: u32, pos: [i64; 3], volume: f32, pitch: f32) {
        let Some(cue) = compute_cue(listener, pos, volume, pitch) else { return };
        let handle = match sound_name(id) {
            Some(name) => self.load_sound(name),
            None => 0,
        };
        self.play_handle(handle, &cue);
    }

    /// `GameController::playSoundAtListener` 0x00484320: at the listener, volume and pitch 1.
    pub fn play_sound_at_listener(&mut self, listener: &Listener, id: u32) {
        self.play_sound(listener, id, listener.pos, 1.0, 1.0);
    }

    /// One server sound record, as `update` plays the ServerUpdate sound list (0x00495630).
    /// Returns whether the record starts the camera shake.
    pub fn play_record(&mut self, listener: &Listener, record: &cw_net::packet::Sound) -> bool {
        let r = SoundRecord::from_net(record);
        self.play_sound(listener, r.id, r.pos, r.volume, r.pitch);
        r.shakes_camera()
    }
}

/// The checks `loadSound` makes through FindChunk (0x00623100): a `RIFF` chunk whose form type
/// is `WAVE`.
pub fn is_riff_wave(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity_listener() -> Listener {
        let mut m = [0.0f32; 16];
        m[0] = 1.0;
        m[5] = 1.0;
        m[10] = 1.0;
        m[15] = 1.0;
        Listener { pos: [0; 3], view_proj: m, sound_volume: 100 }
    }

    #[test]
    fn the_table_has_every_id() {
        assert_eq!(SOUND_NAMES.len(), 101);
        assert_eq!(sound_name(0), Some("hit.wav"));
        assert_eq!(sound_name(0x51), Some("Explosion.wav"));
        assert_eq!(sound_name(0x64), Some("owl2.wav"));
        assert_eq!(sound_name(0x65), None);
    }

    #[test]
    fn falloff_and_pan() {
        let l = identity_listener();
        let at = |blocks: [f32; 3]| blocks.map(|b| (b * 65536.0) as i64);
        // At the listener: full gain, times soundVolume * 0.01.
        let c = compute_cue(&l, at([0.0, 0.0, 0.0]), 1.0, 1.0).unwrap();
        assert_eq!(c.volume, 1.0);
        // 50 blocks away: half.
        let c = compute_cue(&l, at([30.0, 40.0, 0.0]), 0.8, 1.5).unwrap();
        assert!((c.volume - 0.4).abs() < 1e-6);
        assert_eq!(c.pitch, 1.5);
        // x = 30, w = 1: -3 clamps to -1.
        assert_eq!(c.pan, -1.0);
        // Just past 100 blocks: nothing.
        assert!(compute_cue(&l, at([100.01, 0.0, 0.0]), 1.0, 1.0).is_none());
        // Exactly 100 blocks is still played, at zero gain.
        assert_eq!(compute_cue(&l, at([0.0, 100.0, 0.0]), 1.0, 1.0).unwrap().volume, 0.0);
        let c = compute_cue(&l, at([2.0, 0.0, 0.0]), 1.0, 1.0).unwrap();
        assert!((c.pan + 0.2).abs() < 1e-6);
        // w = 0 gives -inf, clamped to -1; NaN also becomes -1.
        let mut zero_w = l;
        zero_w.view_proj[15] = 0.0;
        assert_eq!(compute_cue(&zero_w, at([0.0, 0.0, 0.0]), 1.0, 1.0).unwrap().pan, -1.0);
        // soundVolume scales.
        let quiet = Listener { sound_volume: 30, ..l };
        assert!((compute_cue(&quiet, at([0.0; 3]), 1.0, 1.0).unwrap().volume - 0.3).abs() < 1e-6);
    }

    #[test]
    fn sound_records() {
        let mut raw = [0u8; 0x18];
        raw[0..4].copy_from_slice(&1.5f32.to_le_bytes());
        raw[4..8].copy_from_slice(&(-2.25f32).to_le_bytes());
        raw[8..12].copy_from_slice(&10.0f32.to_le_bytes());
        raw[0xc..0x10].copy_from_slice(&0x51u32.to_le_bytes());
        raw[0x10..0x14].copy_from_slice(&0.9f32.to_le_bytes());
        raw[0x14..0x18].copy_from_slice(&0.5f32.to_le_bytes());
        let r = SoundRecord::from_net(&cw_net::packet::Sound(raw));
        assert_eq!(r.pos, [98304, -147456, 655360]);
        assert_eq!((r.id, r.pitch, r.volume), (0x51, 0.9, 0.5));
        assert!(r.shakes_camera());
        // A silent engine without a database plays nothing and does not panic.
        let mut e = AudioEngine::silent(None);
        assert!(!e.centred_audio);
        e.play_record(&identity_listener(), &cw_net::packet::Sound(raw));
        assert_eq!(e.load_sound("hit.wav"), 0);
    }

    #[test]
    fn riff_wave_check() {
        assert!(is_riff_wave(b"RIFF\x24\0\0\0WAVEfmt "));
        assert!(!is_riff_wave(b"RIFX\x24\0\0\0WAVEfmt "));
        assert!(!is_riff_wave(b"RIFF"));
    }

    /// The `fmt ` sample rate and the `data` byte count over the block align: what XAudio2
    /// plays from the WAVEFORMATEX and XAUDIO2_BUFFER `loadSound` builds (0x00623a60).
    fn riff_rate_and_frames(b: &[u8]) -> (u32, usize) {
        let (mut pos, mut rate, mut align, mut data) = (12usize, 0u32, 0u16, 0usize);
        while pos + 8 <= b.len() {
            let sz = u32::from_le_bytes(b[pos + 4..pos + 8].try_into().unwrap()) as usize;
            match &b[pos..pos + 4] {
                b"fmt " => {
                    rate = u32::from_le_bytes(b[pos + 12..pos + 16].try_into().unwrap());
                    align = u16::from_le_bytes(b[pos + 20..pos + 22].try_into().unwrap());
                }
                b"data" => data = sz,
                _ => {}
            }
            pos += 8 + sz + (sz & 1);
        }
        (rate, data / usize::from(align.max(1)))
    }

    /// Needs `game/data2.db`: kira decodes every blob at the rate and length its header gives,
    /// so a frequency ratio of 1.0 plays at the original speed.
    #[test]
    fn blobs_decode_at_their_own_rate_and_length() {
        let dir = std::env::var("CW_GAME_DIR").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../game").to_string());
        let Ok(db) = AssetDb::open(format!("{dir}/data2.db")) else { return };
        let mut bad = Vec::new();
        for name in SOUND_NAMES.iter().copied().collect::<std::collections::BTreeSet<_>>() {
            let Ok(b) = db.get(name) else { continue };
            let (rate, frames) = riff_rate_and_frames(&b);
            let Ok(d) = kira::sound::static_sound::StaticSoundData::from_cursor(std::io::Cursor::new(b)) else { continue };
            if d.sample_rate != rate || d.frames.len() != frames {
                bad.push(format!("{name}: header {rate} Hz {frames} frames, kira {} Hz {} frames", d.sample_rate, d.frames.len()));
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");
    }

    /// Needs `game/data2.db`: every id's blob decodes as a WAV kira accepts.
    #[test]
    fn every_blob_loads_when_the_game_is_present() {
        let dir = std::env::var("CW_GAME_DIR").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../game").to_string());
        let Ok(db) = AssetDb::open(format!("{dir}/data2.db")) else { return };
        let mut report = Vec::new();
        for (i, name) in SOUND_NAMES.iter().enumerate() {
            match db.get(name) {
                Err(e) => report.push(format!("{i:#x} {name}: {e}")),
                Ok(b) if !is_riff_wave(&b) => report.push(format!("{i:#x} {name}: not RIFF/WAVE")),
                Ok(b) => {
                    if let Err(e) = kira::sound::static_sound::StaticSoundData::from_cursor(std::io::Cursor::new(b)) {
                        report.push(format!("{i:#x} {name}: {e}"));
                    }
                }
            }
        }
        // Three names of the switch have no blob (the key lookup is case-sensitive and the
        // database holds `Fireball.wav`): the original plays nothing for them either.
        assert_eq!(
            report,
            [
                "0x1c cry.wav: blob \"cry.wav\" not found",
                "0x26 fireball.wav: blob \"fireball.wav\" not found",
                "0x60 bird3.wav: blob \"bird3.wav\" not found",
            ]
        );
        let mut e = AudioEngine::silent(Some(db));
        assert_eq!(e.load_sound("cry.wav"), 0);
        // Repeated names share a handle; handles count from 1 in load order.
        assert_eq!(e.load_sound("hit.wav"), 1);
        assert_eq!(e.load_sound("blade1.wav"), 2);
    }
}
