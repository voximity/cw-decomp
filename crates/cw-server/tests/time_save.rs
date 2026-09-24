//! The tick's `time` blob (0x0053254d, every ten seconds of ticks in a named world) is a
//! SQLite write. The client runs the tick on its frame thread under the world lock, so it asks
//! for the write to be handed back ([`TickCtx::deferred_saves`]) and does it on a worker; the
//! dedicated server writes it in place.

use std::collections::BTreeMap;

use cw_net::ServerUpdate;
use cw_server::server::{TickCtx, world_tick_with};
use cw_world::World;
use cw_world::save::{DeferredPut, TIME_KEY};

fn named_world() -> World {
    let mut w = World::new(1);
    w.has_name = true;
    w.attach_save(cw_formats::SaveDb::in_memory().unwrap());
    w
}

fn tick(world: &mut World, deferred: Option<&mut Vec<DeferredPut>>) {
    let mut clock = cw_sim::day::Clock::default();
    let mut projectiles = Vec::new();
    let mut ctx = TickCtx { clock: &mut clock, projectiles: &mut projectiles, verbose: false, log_regions: false, deferred_saves: deferred };
    let (mut entities, mut states) = (BTreeMap::new(), BTreeMap::new());
    let mut out = ServerUpdate::default();
    // 10 000 ms of play crosses the ten-second mark: the blob is due.
    world_tick_with(&mut ctx, world, &mut entities, &mut states, 10_000, Vec::new(), Vec::new(), Vec::new(), &mut Vec::new(), &mut out, &mut |_, _, _, _| {});
}

#[test]
fn without_a_sink_the_tick_writes_the_time_blob_itself() {
    let mut w = named_world();
    tick(&mut w, None);
    assert_eq!(w.saved_blob(TIME_KEY), Some(w.time_blob()));
}

#[test]
fn with_a_sink_the_tick_hands_the_write_back() {
    let mut w = named_world();
    let mut puts = Vec::new();
    tick(&mut w, Some(&mut puts));
    assert_eq!(w.saved_blob(TIME_KEY), None, "written inside the tick");
    assert_eq!(puts.len(), 1);
    assert_eq!(puts[0].key(), TIME_KEY);
    let expected = w.time_blob();
    // The write can run on another thread, after the world has moved on.
    let put = puts.pop().unwrap();
    std::thread::spawn(move || put.write().unwrap()).join().unwrap();
    assert_eq!(w.saved_blob(TIME_KEY), Some(expected));
}

#[test]
fn an_unnamed_world_saves_no_time() {
    let mut w = World::new(1);
    w.has_name = false;
    w.attach_save(cw_formats::SaveDb::in_memory().unwrap());
    let mut puts = Vec::new();
    tick(&mut w, Some(&mut puts));
    assert!(puts.is_empty());
}
