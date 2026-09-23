//! Tests of the gather on synthetic scenes.

use super::*;

/// Model tables with nothing in them but a particle cube.
struct NoModels;

impl SceneModels for NoModels {
    fn model(&self, _: i32) -> Option<ModelInfo> {
        None
    }
    fn prop_model(&self, _: u32) -> Option<ModelInfo> {
        None
    }
    fn prop_model_count(&self) -> usize {
        0
    }
    fn item_model(&self, _: &Item) -> Option<ModelInfo> {
        None
    }
    fn voxels(&self, _: ModelRef) -> Option<VoxelGrid<'_>> {
        None
    }
    fn particle_cube(&self) -> ModelRef {
        1
    }
}

/// Planes every point is inside of (`0·x + 0·y + 0·z + 1 >= 0`).
const ALL_VISIBLE: [[f32; 4]; 6] = [[0.0, 0.0, 0.0, 1.0]; 6];

const B: i64 = 65536;

fn put_f32(e: &mut EntityData, o: usize, v: f32) {
    e.0[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

fn put_i64(e: &mut EntityData, o: usize, v: i64) {
    e.0[o..o + 8].copy_from_slice(&v.to_le_bytes());
}

/// A living creature at `pos` (blocks), facing yaw 0, 2 blocks tall.
fn creature(pos: [i64; 3]) -> EntityData {
    let mut e = EntityData::ZERO;
    put_i64(&mut e, 0, pos[0] * B);
    put_i64(&mut e, 8, pos[1] * B);
    put_i64(&mut e, 0x10, pos[2] * B);
    put_f32(&mut e, 0x78, 2.0);
    put_f32(&mut e, 0x15c, 100.0);
    e
}

/// One chunk (0, 0) in a 1x1 window, visible, with `props`.
fn one_chunk(props: Vec<Prop>) -> SceneState {
    let mut gc = SceneState::default();
    gc.camera.position = [16 * B, 16 * B, 20 * B];
    gc.previous_frustum = ALL_VISIBLE;
    gc.window = ChunkWindow { origin: [0, 0], size: 1 };
    gc.chunks = vec![SceneChunk {
        coords: [0, 0],
        has_buffers: true,
        aabb_min: [0, 0, 0],
        aabb_max: [32 * B, 32 * B, 32 * B],
        center: [16 * B, 16 * B, 16 * B],
        distance_key: 1.0e9,
        props,
    }];
    gc
}

fn torch(pos: [i64; 3]) -> Prop {
    Prop { kind: 0x10, x: pos[0] * B, y: pos[1] * B, z: pos[2] * B, f2c: [1.0, 0.5, 0.2], flags: 2 | 1, ..Prop::NEW }
}

#[test]
fn light_gather_torch_prop_and_light_buff() {
    let mut gc = one_chunk(vec![torch([10, 10, 16])]);
    gc.clock_ms = 1234;
    gc.player = 7;
    let world = World::new(1);
    let mut entities = BTreeMap::new();
    entities.insert(7, creature([20, 20, 16]));
    let mut states = BTreeMap::new();
    let mut st = CreatureState::default();
    st.riding.render_pos = [20 * B, 20 * B, 16 * B];
    let mut buff = [0u8; 0x18];
    buff[0] = 9;
    st.buffs.push(buff);
    states.insert(7, st);

    let mut rng = MsvcRand::new(3);
    let mut sounds = Vec::new();
    let d = daylight(gc.time_of_day_ms);
    let (emitters, per_chunk) = chunk_walk(&mut gc, d, 16, &mut rng, &mut sounds);
    // The walk refreshed the key of the visible chunk: centre (16,16,16), camera (16,16,20).
    assert_eq!(gc.chunks[0].distance_key, 16.0);
    assert_eq!(per_chunk[0].len(), 1);
    assert!(per_chunk[0][0].emits_light);
    // The torch: n = 33, radius cos(1234·0.01 + 33)·0.5 + 15, its colour, not dimmed (kind 0x10).
    assert_eq!(emitters.len(), 1);
    let expected_r = (cw_math::cos(((1234.0f32) * 0.01 + 33.0) as f64) as f32) * 0.5 + 15.0;
    assert_eq!(emitters[0].radius, expected_r);
    assert_eq!(emitters[0].color, [1.0, 0.5, 0.2]);
    assert_eq!(emitters[0].position, [10 * B, 10 * B, 16 * B]);
    // (16-10)² + (16-10)² + (20-16)² = 88 blocks².
    assert_eq!(emitters[0].distance, 88.0);

    let models = NoModels;
    let ctx = SceneCtx {
        world: &world,
        entities: &entities,
        states: &states,
        gc: &gc,
        models: &models,
        projectiles: &[],
        planes: ALL_VISIBLE,
        daylight: d,
        underwater: false,
        world_material: [1.0, 1.0, 1.0, d],
        draw_distance: 122.4,
        prop_far: 20.0,
        prop_near: 18.0,
        static_far: 36.72,
        static_near: 33.048,
        particle_range: 24.48,
        dt_ms: 16,
    };
    let mut timer = 0;
    let (all, dynamic) = gather_lights(&ctx, emitters, &mut timer);
    // The buff-9 light at the hand: Rz(0) · (0.5, 1, 2·0.3) from the render position.
    assert_eq!(dynamic.len(), 1);
    let hand = add3([20 * B, 20 * B, 16 * B], fixed_from_float([0.5, 1.0, 2.0 * 0.3]));
    assert_eq!(dynamic[0].position, hand);
    assert_eq!(dynamic[0].radius, 16.0);
    assert_eq!(dynamic[0].color, [0.6, 0.4, 0.0]);
    // Sorted near → far: the creature's light (4.5² + 5² + 3.4² ≈ 56.8) before the torch (88),
    // although the torch was gathered first.
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].position, hand);
    assert_eq!(all[1].radius, expected_r);
    assert!(all[0].distance < all[1].distance);
    assert_eq!(timer, 16);
}

#[test]
fn light_gather_sorts_by_distance_and_skips_dead_creatures() {
    let gc = one_chunk(Vec::new());
    let world = World::new(1);
    let mut entities = BTreeMap::new();
    // Two creatures carrying torches (weapon sub type 0x14 in slot 6), the far one first in
    // map order; a third one is dead.
    let mut near = creature([16, 17, 20]);
    let mut far = creature([100, 100, 20]);
    let mut dead = creature([16, 16, 20]);
    for e in [&mut near, &mut far, &mut dead] {
        e.0[0x2f0 + 6 * 0x118 + 1] = 0x14;
    }
    put_f32(&mut dead, 0x15c, 0.0);
    entities.insert(1, far);
    entities.insert(2, near);
    entities.insert(3, dead);
    let states = BTreeMap::new();
    let models = NoModels;
    let ctx = SceneCtx {
        world: &world,
        entities: &entities,
        states: &states,
        gc: &gc,
        models: &models,
        projectiles: &[],
        planes: ALL_VISIBLE,
        daylight: 1.0,
        underwater: false,
        world_material: [1.0; 4],
        draw_distance: 122.4,
        prop_far: 20.0,
        prop_near: 18.0,
        static_far: 36.72,
        static_near: 33.048,
        particle_range: 24.48,
        dt_ms: 16,
    };
    let mut timer = 95;
    let (all, dynamic) = gather_lights(&ctx, Vec::new(), &mut timer);
    assert_eq!(dynamic.len(), 2, "the dead creature carries no light");
    assert_eq!(dynamic[0].position[0] / B, 100, "gather order is map order");
    assert_eq!(all[0].position[0] / B, 16, "the sort puts the near one first");
    assert!(all.iter().all(|l| l.radius == 32.0 && l.color == [1.2, 1.0, 0.5]));
    // The 100 ms timer resets once past 100.
    assert_eq!(timer, 0);
}

fn tree(kind: u32) -> Prop {
    Prop { kind, x: 0, y: 0, z: 0, ..Prop::NEW }
}

/// Replays `n` rand() draws.
fn replay(seed: u32, n: usize) -> MsvcRand {
    let mut r = MsvcRand::new(seed);
    for _ in 0..n {
        r.rand();
    }
    r
}

#[test]
fn ambient_sound_one_second_boundary() {
    // Position (0, 0): block y = 333 (the +333-block offset), phase 333.
    let noon = 43_200_000;
    // No crossing: 333 + 0 → 333 + 100 stays in second 0: no rand() drawn.
    let mut rng = MsvcRand::new(11);
    assert_eq!(ambient_sound(&tree(0x3d), noon, 0, 100, &mut rng), None);
    assert_eq!(rng.rand(), MsvcRand::new(11).rand());
    // Crossing at clock 666, dt 1: (333 + 666) / 1000 = 0, (334 + 666) / 1000 = 1.
    for seed in 0..200u32 {
        let mut rng = MsvcRand::new(seed);
        let first = MsvcRand::new(seed).rand();
        let cue = ambient_sound(&tree(0x3d), noon, 666, 1, &mut rng);
        if first % 10 == 0 {
            let cue = cue.expect("a bird sings");
            assert_eq!(cue.id, 0x5e);
            assert_eq!(cue.volume, 0.1);
            let mut r = replay(seed, 1);
            assert_eq!(cue.pitch, (r.rand() as f32) * 0.2 / 32767.0 + 0.9);
            assert_eq!(rng.rand(), replay(seed, 2).rand());
        } else {
            assert_eq!(cue, None);
            assert_eq!(rng.rand(), replay(seed, 1).rand());
        }
    }
    // One millisecond earlier the next frame does not cross.
    let mut rng = MsvcRand::new(0);
    assert_eq!(ambient_sound(&tree(0x3d), noon, 665, 1, &mut rng), None);
}

#[test]
fn ambient_owl_at_night_draws_pitch_then_id() {
    let night = 3_600_000;
    let seed = (0..1000u32).find(|&s| MsvcRand::new(s).rand() % 10 == 0).unwrap();
    let mut rng = MsvcRand::new(seed);
    let cue = ambient_sound(&tree(0x3d), night, 666, 1, &mut rng).unwrap();
    let mut r = replay(seed, 1);
    let pitch = (r.rand() as f32) * 0.2 / 32767.0 + 0.5;
    let id = 0x63 + (r.rand() % 2) as u32;
    assert_eq!(cue, SoundCue { id, pos: [0; 3], volume: 0.1, pitch });
    // Birds (0x3e) are silent at night and not crickets either before noon... but 3.6e6 is
    // before noon, so nothing at all and no draw.
    let mut rng = MsvcRand::new(seed);
    assert_eq!(ambient_sound(&tree(0x3e), night, 666, 1, &mut rng), None);
    assert_eq!(rng.rand(), MsvcRand::new(seed).rand());
}

#[test]
fn ambient_crickets_five_second_boundary() {
    // Evening: 20:00 = 72 000 000 ms, still "day" for birds, so a grass prop (kind 2) is
    // tested for crickets on 5 s boundaries: phase 333, crossing at clock 4666.
    let evening = 72_000_000;
    let seed = (0..100_000u32).find(|&s| MsvcRand::new(s).rand() % 70 == 0).unwrap();
    let mut rng = MsvcRand::new(seed);
    let cue = ambient_sound(&tree(2), evening, 4666, 1, &mut rng).unwrap();
    assert_eq!(cue.id, 0x61);
    assert_eq!(cue.volume, 0.05);
    let mut rng = MsvcRand::new(seed);
    assert_eq!(ambient_sound(&tree(0), evening, 4666, 1, &mut rng).unwrap().id, 0x62);
    // 666 crosses a 1 s but not a 5 s boundary: no draw.
    let mut rng = MsvcRand::new(seed);
    assert_eq!(ambient_sound(&tree(2), evening, 666, 1, &mut rng), None);
    assert_eq!(rng.rand(), MsvcRand::new(seed).rand());
    // Morning (before noon): no crickets, no draw.
    let mut rng = MsvcRand::new(seed);
    assert_eq!(ambient_sound(&tree(2), 30_000_000, 4666, 1, &mut rng), None);
    assert_eq!(rng.rand(), MsvcRand::new(seed).rand());
}

#[test]
fn chunk_walk_sounds_need_the_chunk_within_64_blocks() {
    let seed = (0..1000u32).find(|&s| MsvcRand::new(s).rand() % 10 == 0).unwrap();
    let mut gc = one_chunk(vec![tree(0x3d)]);
    gc.clock_ms = 666;
    // Far camera: the key becomes ≥ 4096 and the tree stays silent.
    gc.camera.position = [16 * B, 16 * B, 100 * B];
    let mut rng = MsvcRand::new(seed);
    let mut sounds = Vec::new();
    chunk_walk(&mut gc, 1.0, 1, &mut rng, &mut sounds);
    assert!(gc.chunks[0].distance_key >= 4096.0);
    assert!(sounds.is_empty());
    // Near camera.
    gc.camera.position = [16 * B, 16 * B, 20 * B];
    let mut rng = MsvcRand::new(seed);
    chunk_walk(&mut gc, 1.0, 1, &mut rng, &mut sounds);
    assert_eq!(sounds.len(), 1);
    assert_eq!(sounds[0].id, 0x5e);
}

#[test]
fn leaves_fall_on_second_boundaries_only() {
    let mut gc = one_chunk(vec![Prop { kind: 0x3c, x: 5 * B, y: 5 * B, z: 20 * B, f2c: [0.2, 0.6, 0.1], ..Prop::NEW }]);
    gc.view_distance = 100.0;
    let world = World::new(1);
    let entities = BTreeMap::new();
    let states = BTreeMap::new();
    let models = NoModels;
    let seed = (0..1000u32).find(|&s| MsvcRand::new(s).rand() % 16 == 0).unwrap();
    for (clock, falls) in [(999, true), (500, false)] {
        gc.clock_ms = clock;
        let ctx = SceneCtx {
            world: &world,
            entities: &entities,
            states: &states,
            gc: &gc,
            models: &models,
            projectiles: &[],
            planes: ALL_VISIBLE,
            daylight: 1.0,
            underwater: false,
            world_material: [1.0; 4],
            draw_distance: 122.4,
            prop_far: 20.0,
            prop_near: 18.0,
            static_far: 36.72,
            static_near: 33.048,
            particle_range: 24.48,
            dt_ms: 1,
        };
        let mut particles = ParticleSystem::new();
        let mut rng = MsvcRand::new(seed);
        let mut far = Vec::new();
        terrain_props(&ctx, &[0], &mut particles, &mut rng, &mut far);
        if falls {
            assert_eq!(particles.len(), 1);
            let p = particles.particles[0];
            assert_eq!(p.kind, 4);
            assert_eq!(p.color, [0.2, 0.6, 0.1, 1.0]);
            assert_eq!(p.lifetime, 20_000);
            assert_eq!(p.velocity[2], -2.0);
            // 1 test + 5 for the leaf.
            assert_eq!(rng.rand(), replay(seed, 6).rand());
        } else {
            assert!(particles.is_empty());
            assert_eq!(rng.rand(), MsvcRand::new(seed).rand());
        }
    }
}

#[test]
fn gui_visibility_hides_the_hud_under_panels() {
    let mut g = GuiVisibility { w8a0: true, ..GuiVisibility::default() };
    g.apply();
    assert!(g.hud);
    assert!(!g.w8a0);
    g.w890 = true;
    g.apply();
    assert!(!g.hud);
    assert!(!g.hides_creatures());
    g.w88c = true;
    assert!(g.hides_creatures());
}

#[test]
fn spirit_colour_averages_the_elemental_cubes() {
    let mut item = [0u8; 0x118];
    item[0x114] = 3;
    item[0x14 + 3] = 0x80; // fire
    item[0x14 + 8 + 3] = 0x82; // ice
    item[0x14 + 16 + 3] = 0x10; // not elemental
    let c = spirit_color(&item);
    // Fire (1.5, 0.75, 0.15, 3) and ice (0.45, 0.75, 1.5, 3), halved.
    assert_eq!(c, [(1.5 + 0.45) * 0.5, (0.75 + 0.75) * 0.5, (0.15f32 + 1.5) * 0.5, 3.0]);
    assert_eq!(spirit_color(&[0u8; 0x118]), [0.0; 4]);
}

#[test]
fn prop_fade_between_near_and_far() {
    let gc = one_chunk(Vec::new());
    let world = World::new(1);
    let entities = BTreeMap::new();
    let states = BTreeMap::new();
    let models = NoModels;
    let ctx = SceneCtx {
        world: &world,
        entities: &entities,
        states: &states,
        gc: &gc,
        models: &models,
        projectiles: &[],
        planes: ALL_VISIBLE,
        daylight: 1.0,
        underwater: false,
        world_material: [1.0, 1.0, 1.0, 0.5],
        draw_distance: 122.4,
        prop_far: 20.0,
        prop_near: 10.0,
        static_far: 36.72,
        static_near: 33.048,
        particle_range: 24.48,
        dt_ms: 1,
    };
    let p = Prop { kind: 1, scale: 1.0, f28: 51.0, ..Prop::NEW };
    let model = ModelInfo { handle: 5, size: [4, 6, 8] };
    let d = draw_prop(&ctx, &p, model, 100.0, 20.0, 10.0, 1).unwrap();
    assert_eq!(d.alpha, 1.0);
    assert_eq!(d.material, [1.0, 1.0, 1.0, (51.0f32 / 255.0) * 0.5]);
    // Centred on x/y: the model corner at (-2, -3, 0) from the prop position.
    assert_eq!(transform_point(&d.world, [0.0; 3]), [-2.0, -3.0, 0.0]);
    let d = draw_prop(&ctx, &p, model, 225.0, 20.0, 10.0, 1).unwrap();
    assert_eq!(d.alpha, 0.5);
    // Lit props are full bright.
    let lit = Prop { flags: 3, ..p.clone() };
    assert_eq!(draw_prop(&ctx, &lit, model, 100.0, 20.0, 10.0, 1).unwrap().material[3], 1.0);
    // A swaying prop keeps its base on the ground: the model origin maps to the same point.
    let sway = Prop { flags: 2 | 4, ..p };
    let d = draw_prop(&ctx, &sway, model, 100.0, 20.0, 10.0, 1).unwrap();
    let base = transform_point(&d.world, [2.0, 3.0, 0.0]);
    assert!(base.iter().all(|v| v.abs() < 1e-5), "{base:?}");
}
