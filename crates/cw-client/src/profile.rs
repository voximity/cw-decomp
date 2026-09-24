//! Frame-time diagnostics behind `CW_CLIENT_STATS=1` (the port's own; the original has none).
//!
//! The main thread accumulates the time of each phase of a frame and the time it waited for
//! each shared lock in thread-local counters ([`scope`], [`add`]); `app::App::frame` takes them
//! once per frame ([`FrameLog::end_frame`]), logs every frame over [`SLOW_FRAME_MS`] with its
//! breakdown, prints a summary every five seconds and a total at exit. Worker threads report
//! long lock holds and long units of work with [`worker_note`].
//!
//! `CW_CLIENT_TRACE=<file.csv>` also records every frame to that file, one row per frame
//! ([`trace_header`]): the time since start, the frame's wall time, the `dt` the update was
//! given and every phase and lock wait, all in ms. It turns the counters on by itself.
//!
//! `CW_CLIENT_STATS_WAIT_MS=<ms>` records every worker hold of the `World` lock and logs each
//! frame-thread wait for it over `<ms>` with the holds that overlapped it, plus per-holder
//! totals at exit ([`attribute_wait`]).
//!
//! Everything is a no-op (one relaxed load of a cached flag) when neither variable is set.

use std::cell::RefCell;
use std::io::Write;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// A frame slower than this is logged with its breakdown (two 60 Hz frames).
pub const SLOW_FRAME_MS: f64 = 33.0;

/// A worker unit of work (or lock hold) longer than this is logged (`CW_CLIENT_STATS_NOTE_MS`
/// overrides it).
pub const WORKER_NOTE_MS: f64 = 8.0;

fn note_ms() -> f64 {
    static MS: OnceLock<f64> = OnceLock::new();
    *MS.get_or_init(|| std::env::var("CW_CLIENT_STATS_NOTE_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(WORKER_NOTE_MS))
}

/// The process's first use of the diagnostics (the time base of the log lines).
fn epoch() -> Instant {
    static T: OnceLock<Instant> = OnceLock::new();
    *T.get_or_init(Instant::now)
}

/// Whether `CW_CLIENT_STATS` or `CW_CLIENT_TRACE` is set (read once).
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("CW_CLIENT_STATS").is_some() || std::env::var_os("CW_CLIENT_TRACE").is_some())
}

/// The CSV header of `CW_CLIENT_TRACE`.
pub fn trace_header() -> String {
    let mut h = String::from("t_s,frame_ms,dt_ms");
    for n in NAMES {
        h.push(',');
        h.push_str(n);
    }
    h
}

/// One `CW_CLIENT_TRACE` row: `t_s` since start, the frame's wall time, the update's `dt` and
/// the phase times.
fn trace_row(t_s: f64, total: Duration, dt: i32, acc: &[Duration; PHASES]) -> String {
    let mut r = format!("{t_s:.6},{:.3},{dt}", ms(total));
    for d in acc {
        r.push_str(&format!(",{:.3}", ms(*d)));
    }
    r
}

/// The measured pieces of a frame. The `Wait*` entries are lock waits, which also count in the
/// phase they happen in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Phase {
    /// Window messages, scripted input, cursor grab, gamepad, slot 6.
    Input,
    /// `update` head: world loads finished, the fog distance.
    UpdateHead,
    /// The UI frame (`ui_frame`).
    Ui,
    /// The world tick in 20 ms slices (`world_tick`).
    Tick,
    /// The received lists (`received::apply`).
    Net,
    /// The gameplay after the tick, particles, the remesh marks.
    Player,
    /// Sounds, effects, the character save, the thread inputs (lock B).
    Send,
    /// `render` up to and including `gather_scene` (ring snapshot, scene state).
    Gather,
    /// The creature poses.
    Poses,
    /// The GUI tessellation (`assets::gui_frame`).
    Gui,
    /// `passes::build_frame` and the map side effects.
    BuildFrame,
    /// The sink's uploads: GUI stream, textures, glyph atlas, chunk buffers, model meshes.
    Uploads,
    /// Acquiring the back buffer.
    Acquire,
    /// Recording the frame (executor prepare, stream uploads, encode).
    Encode,
    /// `queue.submit`.
    Submit,
    /// `present`.
    Present,
    /// The FPS-limit sleep.
    Sleep,
    /// Waiting for the `World` lock (A, `ClientShared::world`).
    WaitWorld,
    /// Waiting for the chunk ring lock (C).
    WaitRing,
    /// Waiting for the world request lock (B).
    WaitCs,
    /// Waiting for the map tiles lock.
    WaitTiles,
}

/// Number of [`Phase`]s.
pub const PHASES: usize = Phase::WaitTiles as usize + 1;

const NAMES: [&str; PHASES] = [
    "input", "head", "ui", "tick", "net", "player", "send", "gather", "poses", "gui", "build", "uploads", "acquire", "encode", "submit",
    "present", "sleep", "wWorld", "wRing", "wCs", "wTiles",
];

thread_local! {
    static ACC: RefCell<[Duration; PHASES]> = const { RefCell::new([Duration::ZERO; PHASES]) };
}

/// Adds `d` to this thread's counter of `p`.
pub fn add(p: Phase, d: Duration) {
    if enabled() {
        ACC.with(|a| a.borrow_mut()[p as usize] += d);
    }
}

/// Times a lock acquisition (or anything) into `p` on this thread.
pub fn wait<T>(p: Phase, f: impl FnOnce() -> T) -> T {
    if !enabled() {
        return f();
    }
    let t = Instant::now();
    let r = f();
    add(p, t.elapsed());
    r
}

/// Adds the time until it is dropped to `p`.
pub struct Scope(Phase, Option<Instant>);

impl Drop for Scope {
    fn drop(&mut self) {
        if let Some(t) = self.1 {
            add(self.0, t.elapsed());
        }
    }
}

/// A [`Scope`] for `p`.
pub fn scope(p: Phase) -> Scope {
    Scope(p, enabled().then(Instant::now))
}

fn take() -> [Duration; PHASES] {
    ACC.with(|a| std::mem::replace(&mut *a.borrow_mut(), [Duration::ZERO; PHASES]))
}

/// A worker's unit of work: logged when longer than [`WORKER_NOTE_MS`].
pub fn worker_note(what: &str, d: Duration) {
    if enabled() {
        let ms = d.as_secs_f64() * 1000.0;
        if ms > note_ms() {
            let end = epoch().elapsed().as_secs_f64();
            eprintln!("cw-client: [{}] {what} {ms:.1} ms (ended at {end:.3} s)", std::thread::current().name().unwrap_or("?"));
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Lock A wait attribution (`CW_CLIENT_STATS_WAIT_MS=<ms>`).

/// With `CW_CLIENT_STATS_WAIT_MS=<ms>` every worker hold of the `World` lock is recorded, and
/// every frame-thread wait for it longer than `<ms>` is logged with the worker holds that
/// overlapped it; the run's totals per holder are printed at exit.
pub fn wait_attribution_ms() -> Option<f64> {
    static MS: OnceLock<Option<f64>> = OnceLock::new();
    *MS.get_or_init(|| if enabled() { std::env::var("CW_CLIENT_STATS_WAIT_MS").ok().and_then(|v| v.parse().ok()) } else { None })
}

#[derive(Clone, Copy)]
struct HoldRec {
    thread: &'static str,
    what: &'static str,
    start: Instant,
    end: Instant,
}

#[derive(Default)]
struct Attribution {
    /// Recent worker holds (the last second or so).
    holds: std::collections::VecDeque<HoldRec>,
    /// Per `(thread, what)`: holds, total held ms, longest hold, the frame waits it overlapped
    /// and the overlap in ms.
    totals: std::collections::BTreeMap<(&'static str, &'static str), [f64; 5]>,
    /// Frame waits over the threshold and their total ms.
    waits: [f64; 2],
}

fn attribution() -> &'static std::sync::Mutex<Attribution> {
    static A: OnceLock<std::sync::Mutex<Attribution>> = OnceLock::new();
    A.get_or_init(Default::default)
}

fn thread_label() -> &'static str {
    thread_local! {
        static NAME: &'static str = Box::leak(std::thread::current().name().unwrap_or("?").to_string().into_boxed_str());
    }
    NAME.with(|n| *n)
}

/// A worker released the `World` lock it took at `start` (unit `what`).
pub fn record_hold(what: &'static str, start: Instant) {
    if wait_attribution_ms().is_none() {
        return;
    }
    let end = Instant::now();
    let held = ms(end - start);
    let mut a = attribution().lock().unwrap_or_else(|e| e.into_inner());
    let rec = HoldRec { thread: thread_label(), what, start, end };
    let t = a.totals.entry((rec.thread, what)).or_default();
    t[0] += 1.0;
    t[1] += held;
    t[2] = t[2].max(held);
    a.holds.push_back(rec);
    while a.holds.front().is_some_and(|h| end.duration_since(h.end) > Duration::from_secs(1)) {
        a.holds.pop_front();
    }
}

/// The frame thread waited for the `World` lock from `start` to `end`.
pub fn attribute_wait(start: Instant, end: Instant) {
    let Some(limit) = wait_attribution_ms() else { return };
    let waited = ms(end - start);
    if waited <= limit {
        return;
    }
    let mut a = attribution().lock().unwrap_or_else(|e| e.into_inner());
    a.waits[0] += 1.0;
    a.waits[1] += waited;
    let overlapping: Vec<HoldRec> = a.holds.iter().filter(|h| h.end > start && h.start < end).copied().collect();
    let mut line = String::new();
    for h in &overlapping {
        let lo = h.start.max(start);
        let hi = h.end.min(end);
        let overlap = ms(hi - lo);
        line.push_str(&format!(" {}:{} {:.2}/{:.2}", h.thread, h.what, overlap, ms(h.end - h.start)));
        if let Some(t) = a.totals.get_mut(&(h.thread, h.what)) {
            t[3] += 1.0;
            t[4] += overlap;
        }
    }
    let at = epoch().elapsed().as_secs_f64();
    eprintln!("cw-client: World wait {waited:.2} ms (ended at {at:.3} s) | overlapping holds (overlap/held ms):{line}");
}

fn attribution_summary() {
    if wait_attribution_ms().is_none() {
        return;
    }
    let a = attribution().lock().unwrap_or_else(|e| e.into_inner());
    eprintln!("cw-client: World waits over {:.1} ms: {} ({:.1} ms total); worker holds:", wait_attribution_ms().unwrap_or(0.0), a.waits[0], a.waits[1]);
    for ((th, what), t) in &a.totals {
        eprintln!(
            "cw-client:   {th}:{what}: {} holds, mean {:.3} ms, max {:.2} ms; in {} waits, {:.1} ms of overlap",
            t[0],
            t[1] / t[0].max(1.0),
            t[2],
            t[3],
            t[4]
        );
    }
}

/// Frame times of the main thread: per-frame breakdowns, summaries, the total.
pub struct FrameLog {
    started: Instant,
    frames: u64,
    /// Frame times of the current summary window, and of the whole run, in ms.
    window: Vec<f64>,
    all: Vec<f64>,
    window_start: Instant,
    /// The phase sums of the current summary window.
    window_phases: [Duration; PHASES],
    /// `CW_CLIENT_TRACE`'s file.
    trace: Option<std::io::BufWriter<std::fs::File>>,
}

impl Default for FrameLog {
    fn default() -> Self {
        let now = Instant::now();
        epoch();
        let trace = std::env::var_os("CW_CLIENT_TRACE").and_then(|p| match std::fs::File::create(&p) {
            Ok(f) => {
                let mut w = std::io::BufWriter::new(f);
                let _ = writeln!(w, "{}", trace_header());
                eprintln!("cw-client: tracing frames to {}", std::path::Path::new(&p).display());
                Some(w)
            }
            Err(e) => {
                eprintln!("cw-client: CW_CLIENT_TRACE {}: {e}", std::path::Path::new(&p).display());
                None
            }
        });
        FrameLog { started: now, frames: 0, window: Vec::new(), all: Vec::new(), window_start: now, window_phases: [Duration::ZERO; PHASES], trace }
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn summary(v: &[f64]) -> String {
    if v.is_empty() {
        return "no frames".into();
    }
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    let mean = s.iter().sum::<f64>() / s.len() as f64;
    let pct = |p: f64| s[((s.len() - 1) as f64 * p) as usize];
    let over = |t: f64| s.iter().filter(|&&x| x > t).count();
    format!(
        "{} frames, mean {:.1} ms, p50 {:.1}, p99 {:.1}, max {:.1}; >33 ms: {}, >50 ms: {}, >100 ms: {}",
        s.len(),
        mean,
        pct(0.5),
        pct(0.99),
        s[s.len() - 1],
        over(33.0),
        over(50.0),
        over(100.0)
    )
}

impl FrameLog {
    /// One frame of `total` done (its update given `dt` ms): the trace row, the slow-frame
    /// line and the five-second summary.
    pub fn end_frame(&mut self, total: Duration, dt: i32) {
        if !enabled() {
            return;
        }
        let acc = take();
        if let Some(w) = self.trace.as_mut() {
            let _ = writeln!(w, "{}", trace_row(epoch().elapsed().as_secs_f64(), total, dt, &acc));
        }
        self.frames += 1;
        let t = ms(total);
        self.window.push(t);
        self.all.push(t);
        for (w, a) in self.window_phases.iter_mut().zip(acc.iter()) {
            *w += *a;
        }
        if t > SLOW_FRAME_MS {
            let mut parts = String::new();
            for (i, d) in acc.iter().enumerate() {
                let v = ms(*d);
                if v >= 0.5 {
                    parts.push_str(&format!(" {} {:.1}", NAMES[i], v));
                }
            }
            eprintln!("cw-client: slow frame {} ended at {:.3} s: {:.1} ms |{}", self.frames, epoch().elapsed().as_secs_f64(), t, parts);
        }
        if self.window_start.elapsed() >= Duration::from_secs(5) {
            let n = self.window.len().max(1) as f64;
            let mut means = String::new();
            for (i, d) in self.window_phases.iter().enumerate() {
                let v = ms(*d) / n;
                if v >= 0.05 {
                    means.push_str(&format!(" {} {:.2}", NAMES[i], v));
                }
            }
            eprintln!(
                "cw-client: frames {:.0}..{:.0} s: {} | mean per frame:{}",
                (self.window_start - self.started).as_secs_f64(),
                self.started.elapsed().as_secs_f64(),
                summary(&self.window),
                means
            );
            self.window.clear();
            self.window_phases = [Duration::ZERO; PHASES];
            self.window_start = Instant::now();
        }
    }

    /// The whole run's summary (at exit), and the trace flushed.
    pub fn finish(&mut self) {
        if let Some(w) = self.trace.as_mut() {
            let _ = w.flush();
        }
        if enabled() {
            eprintln!("cw-client: run of {:.1} s: {}", self.started.elapsed().as_secs_f64(), summary(&self.all));
            attribution_summary();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_counts() {
        let s = summary(&[10.0, 40.0, 60.0, 120.0]);
        assert!(s.contains(">33 ms: 3, >50 ms: 2, >100 ms: 1"), "{s}");
        assert_eq!(NAMES.len(), PHASES);
    }

    /// `CW_CLIENT_TRACE`: one CSV row per frame, the columns of [`trace_header`].
    #[test]
    fn trace_rows_match_the_header() {
        let h = trace_header();
        assert!(h.starts_with("t_s,frame_ms,dt_ms,input,head,ui,tick,"));
        let mut acc = [Duration::ZERO; PHASES];
        acc[Phase::Tick as usize] = Duration::from_micros(4_250);
        acc[Phase::Sleep as usize] = Duration::from_micros(6_000);
        let r = trace_row(1.5, Duration::from_micros(16_700), 17, &acc);
        let cols: Vec<&str> = r.split(',').collect();
        assert_eq!(cols.len(), h.split(',').count());
        assert_eq!(&cols[..3], ["1.500000", "16.700", "17"]);
        assert_eq!(cols[3 + Phase::Tick as usize], "4.250");
        assert_eq!(cols[3 + Phase::Sleep as usize], "6.000");
        assert_eq!(cols[3 + Phase::Input as usize], "0.000");
    }
}
