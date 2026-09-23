//! The generation thread (`generationThread` 0x00549550, note 3.3): one missing zone nearest
//! to a player per iteration, and every second the unloading (and saving) of the zones and
//! regions no player is near (`cw_world::persistence`).
//!
//! Zones are built in this thread's own copy of the world, so the served world stays free for
//! the tick and the send threads during the half second a zone takes. Before each zone the
//! copy pulls the play-time state generation reads (the cells with their mission and monster
//! state, the players, the creature ids); afterwards the finished zone, the regions and
//! climate points created on the way, and the creature removals generation queued go to the
//! served world under a brief lock. The original's generation thread holds no lock at all
//! while it generates (only the request list's critical section, released before
//! `generateZone`): it races the tick on one world and publishes a zone by writing its pointer
//! into the region table. The copy is the memory-safe form of that; the generation algorithms
//! and their order are unchanged.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crate::server::Server;

/// Squared zone distance within which zones are generated and kept (`best` starts at 9).
const RADIUS_SQ: i32 = 9;

pub fn generation_loop(server: Arc<Server>) {
    let mut last_unload = Instant::now();
    let mut generator = server.world.lock().unwrap().generator_copy();
    while server.running.load(Ordering::Relaxed) {
        let players = server.player_zones.lock().unwrap().clone();
        let mut generated = false;
        if !players.is_empty() {
            let pick = {
                let world = server.world.lock().unwrap();
                generator.pull_play_state(&world);
                // The nearest missing zone: `best` carries across players, strict `<`.
                let mut best = RADIUS_SQ;
                let mut pick = None;
                for &(px, py) in &players {
                    for x in (px - 9).max(0)..=(px + 9).min(0xffff) {
                        for y in (py - 9).max(0)..=(py + 9).min(0xffff) {
                            let d = (px - x) * (px - x) + (py - y) * (py - y);
                            if d < best && world.zone(x, y).is_none() {
                                best = d;
                                pick = Some((x, y));
                            }
                        }
                    }
                }
                pick
            };
            if let Some((x, y)) = pick {
                let t = Instant::now();
                // `generateZone` creates the 3x3 regions around the zone first (0x005186e1),
                // with no lock held, so the original's tick can activate a region (and run
                // `spawnCellNpc` on the zones already loaded in it) while the zone is still
                // being built; the zone under construction is not in the table yet and keeps
                // the wandering group it rolls at the end on this thread's `rand`. The regions
                // are published before the zone for the same order of events.
                for dx in -1..=1 {
                    for dy in -1..=1 {
                        generator.create_region(x / 64 + dx, y / 64 + dy);
                    }
                }
                {
                    let mut world = server.world.lock().unwrap();
                    generator.publish_generation(&mut world);
                }
                // `spawnCellNpc` at the end of `generateZone` sees the cells as the tick has
                // them then: a region the tick activated while this zone was being built has
                // its mission monster placed, so the mission zone gets its boss.
                let srv = server.clone();
                generator.generate_zone_hooked(x, y, &mut |_, _| {}, &mut |g| {
                    let world = srv.world.lock().unwrap();
                    g.pull_play_state(&world);
                });
                let zone = generator.remove_zone(x, y);
                println!("Generated zone {x},{y} in {} ms", t.elapsed().as_millis());
                let mut world = server.world.lock().unwrap();
                generator.publish_generation(&mut world);
                if let Some(zone) = zone
                    && world.zone(x, y).is_none()
                {
                    world.insert_zone(zone);
                }
                generated = true;
            }
        }
        // 0x005497d9..0x005499a8: every second (`1000 < now - last`), whether or not any player
        // is connected, the unloading pass (`World::unload_idle`): idle zones saved and dropped
        // (`unloadZone`), regions beyond two regions saved (`saveEntities`) and dropped
        // (`unloadRegion`), climate points beyond four freed (`removeRegion`). The original
        // runs it with no world lock; here it runs under the lock, and the generator copy
        // forgets the same regions and points so that a region needed again is re-created
        // from the save database (`createRegion` phase J) rather than published from the copy.
        if last_unload.elapsed() > Duration::from_millis(1000) {
            last_unload = Instant::now();
            let report = server.world.lock().unwrap().unload_idle(&players);
            for &(rx, ry) in &report.regions {
                generator.forget_region(rx, ry);
            }
            for &(rx, ry) in &report.points {
                generator.remove_region(rx, ry);
            }
            if server.verbose && !report.is_empty() {
                println!("Unloaded {} zones, {} regions, {} climate points.", report.zones.len(), report.regions.len(), report.points.len());
            }
        }
        if !generated {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
