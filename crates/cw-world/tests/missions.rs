//! The missions `activateRegionMissions` rolls for the world-spawn region of seed 26879 when
//! the first player (level 1, standing at the world spawn) arrives: Server.exe announces them
//! in the first ServerUpdate after the join (captured with `tools/oracle/client_probe.py` and
//! decoded by `cw-server`'s `probe_diff` example). The rolls draw from the main thread's
//! `rand` stream as `World::load` leaves it.

mod common;

use common::world_with_models;
use cw_world::seeds::Seeds;
use cw_world::world::PlayerInfo;

const SEED: i32 = 26879;
const SPAWN_ZONE: (i32, i32) = (32800, 32800);
const REGION: (i32, i32) = (512, 512);

#[test]
fn spawn_region_missions_match_the_original() {
    let mut world = world_with_models(SEED);
    // The spawn zone brings the region and its climate points into being.
    world.generate_zone(SPAWN_ZONE.0, SPAWN_ZONE.1);
    let (_, rng) = Seeds::from_seed_with_rng(SEED);
    world.rng = rng;
    // The player as acceptLoop places it: `__ftol2(spawn_f32 * 65536.0f)` per axis, z 0.
    let x = (world.spawn[0] * 65536.0f32) as i64;
    let y = (world.spawn[1] * 65536.0f32) as i64;
    world.players = vec![PlayerInfo { id: 1, level: 1, pos: [x, y, 0] }];
    let (rx, ry) = world.nearest_climate_region((x / 65536) as i32, (y / 65536) as i32);
    assert_eq!((rx, ry), REGION);
    assert!(!world.region(rx, ry).unwrap().missions_active);

    let mut out = Vec::new();
    world.activate_region_missions(rx, ry, &mut out);

    let got: Vec<String> = out
        .iter()
        .map(|m| format!("{} {} {} {} {} {} {} {} {} {} {} {} {}", m.cell_x, m.cell_y, m.words[0], m.words[1], m.words[2], m.words[3], m.words[4], m.b40, m.b41, m.tail[0], m.tail[1], m.tail[2], m.tail[3]))
        .collect();
    let want: Vec<&str> = include_str!("golden/missions_26879.txt").lines().filter(|l| !l.is_empty() && !l.starts_with('#')).collect();
    assert_eq!(got, want);
    assert!(world.region(rx, ry).unwrap().missions_active);
    // The cells keep what the records announced.
    let cell = world.cell(4100, 4102).unwrap();
    assert_eq!((cell.mission.f34, cell.mission.f38, cell.mission.f3c, cell.mission.f48), (5, 43, 1, 19));
    assert_eq!(cell.mission.f4c, (32819i64 << 32) | 32803);
}
