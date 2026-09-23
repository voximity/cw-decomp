//! The creature behaviours (`cube::Behavior` and its subclasses): the list a creature owns at
//! `creature+0x13e4` is a `SequentialBehavior` (0x0041d080) that runs its children in order
//! until one returns true, and zeroes the acceleration when none does. The tick runs it
//! every 32 ms of idle time (`update_creature`). Ported here: `LookAtPlayerBehavior`
//! 0x00414730, `RandomWalkBehavior` 0x0041cbb0 and the target selection of `CombatBehavior`
//! 0x00402f40. Not ported yet: the rest of `CombatBehavior` (chase, attack modes),
//! `SpawnLocationBehavior` 0x00428940, `WalkPathBehavior` 0x004c5e00,
//! `RandomInteractionBehavior` 0x0041bc20 and `CompanionBehavior` 0x00405780, which all need
//! the path finder (0x004dd2e0) and path following (0x004db200).
//!
//! The behaviour draws from the tick thread's `rand`, which the caller has in [`World::rng`].

use std::collections::BTreeMap;

use cw_net::EntityData;
use cw_net::ServerUpdate;
use cw_world::World;

use crate::combat::CreatureState;
use crate::util::{K, f32_at, i64_at, pos_at, set_accel, u16_at, w16, wf32};

/// One behaviour of a creature's list.
#[derive(Debug, Clone)]
pub enum Behavior {
    /// `CombatBehavior` (0x14 bytes; `combat_ai.rs`).
    Combat(crate::combat_ai::CombatState),
    /// `LookAtPlayerBehavior` (no fields).
    LookAtPlayer,
    /// `RandomWalkBehavior`: +4 the milliseconds until the next direction.
    RandomWalk { timer: i32 },
    /// A behaviour the port does not run yet (named for the notes).
    Unported(&'static str),
}

/// The default list the tick gives a creature made from a spawn without its own AI
/// (`World::tick` 0x00535fe3..0x00536130): none for hostile 6; otherwise Combat(20),
/// LookAtPlayer and, unless the class is 0x80..0x87 or the appearance carries flag 0x40,
/// RandomWalk.
pub fn default_behaviors(e: &EntityData) -> Vec<Behavior> {
    if e.0[0x50] == 6 {
        return Vec::new();
    }
    let mut v = vec![Behavior::Combat(crate::combat_ai::CombatState::new(20.0)), Behavior::LookAtPlayer];
    let class = e.0[0x130];
    if !matches!(class, 0x80..=0x87) && u16_at(&e.0, 0x6e) & 0x40 == 0 {
        v.push(Behavior::RandomWalk { timer: 0 });
    }
    v
}

/// The creature's behaviour tree run once (`World::tick` 0x005378e6: `creature+0x13e4`'s
/// virtual update). The tree, the path state and the combat AI fields are taken out of the
/// creature's state while it runs, as the sub-behaviours borrow the state map for the other
/// creatures.
pub fn run_behaviors(world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, id: i64, elapsed: i32, out: &mut ServerUpdate) {
    let (mut root, mut path, mut ai) = {
        let st = states.entry(id).or_default();
        (st.ai_root.take(), std::mem::take(&mut st.path), std::mem::take(&mut st.ai))
    };
    if let Some(node) = root.as_mut() {
        let mut base = |world: &mut World, entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, path: &mut crate::path::PathState, id: i64, elapsed: i32, b: &mut Behavior, out: &mut ServerUpdate| -> bool {
            match b {
                Behavior::Combat(cs) => crate::combat_ai::combat_update(world, entities, states, &mut ai, path, id, elapsed, cs, out),
                Behavior::LookAtPlayer => look_at_player(entities, states, path, id),
                Behavior::RandomWalk { timer } => random_walk(world, entities, id, elapsed, timer),
                Behavior::Unported(_) => false,
            }
        };
        crate::behaviors_more::run_ai(world, entities, states, &mut path, id, elapsed, node, out, &mut base);
    }
    let st = states.entry(id).or_default();
    st.ai_root = root;
    st.path = path;
    st.ai = ai;
}

/// `LookAtPlayerBehavior::update` 0x00414730: the nearest player with HP >= 0 within 8
/// blocks becomes the aim (flag 4, the ray hit set to the difference in blocks), the
/// acceleration is zeroed and the path cleared. Without one flag 4 clears and the next
/// behaviour runs.
fn look_at_player(entities: &mut BTreeMap<i64, EntityData>, states: &mut BTreeMap<i64, CreatureState>, path: &mut crate::path::PathState, id: i64) -> bool {
    let me = pos_at(&entities[&id].0);
    let mut best: Option<[i64; 3]> = None;
    let mut best_d = 64.0f32;
    for (k, o) in entities.iter() {
        if *k == id || o.0[0x50] != 0 || f32_at(&o.0, 0x15c) < 0.0 {
            continue;
        }
        let p = pos_at(&o.0);
        let d = [(p[0].wrapping_sub(me[0])) as f32 * K, (p[1].wrapping_sub(me[1])) as f32 * K, (p[2].wrapping_sub(me[2])) as f32 * K];
        let dsq = d[1] * d[1] + d[0] * d[0] + d[2] * d[2];
        if !(best_d <= dsq) {
            best = Some(p);
            best_d = dsq;
        }
    }
    let e = entities.get_mut(&id).expect("creature");
    let mut flags = u16_at(&e.0, 0x114) & 0xfffb;
    w16(&mut e.0, 0x114, flags);
    if let Some(p) = best
        && !matches!(e.0[0x58], 0x53 | 0x54)
    {
        flags |= 4;
        w16(&mut e.0, 0x114, flags);
        for i in 0..3 {
            wf32(&mut e.0, 0x150 + i * 4, (p[i].wrapping_sub(me[i])) as f32 * K);
        }
        set_accel(e, [0.0; 3]);
        crate::path::clear_path(path, states.entry(id).or_default());
        return true;
    }
    false
}

/// `RandomWalkBehavior::update` 0x0041cbb0: flag 0x40 clears; when the timer runs out the
/// creature accelerates at 10 blocks/s² in a random horizontal direction while within ten
/// blocks of its spawn point, else back towards it, for `rand() % 5000 + 3000` ms, and
/// drops its mode. Always returns true.
fn random_walk(world: &mut World, entities: &mut BTreeMap<i64, EntityData>, id: i64, elapsed: i32, timer: &mut i32) -> bool {
    let e = entities.get_mut(&id).expect("creature");
    let flags = u16_at(&e.0, 0x114) & 0xffbf;
    w16(&mut e.0, 0x114, flags);
    let t = *timer - elapsed;
    *timer = if t < 0 { 0 } else { t };
    if *timer == 0 {
        let pos = pos_at(&e.0);
        let spawn = [i64_at(&e.0, 0x1b0), i64_at(&e.0, 0x1b8), i64_at(&e.0, 0x1c0)];
        let dx = pos[0].wrapping_sub(spawn[0]);
        let dy = pos[1].wrapping_sub(spawn[1]);
        let dsq = dx.wrapping_mul(dx) / 65536 + dy.wrapping_mul(dy) / 65536;
        if dsq <= 0x640000 {
            let x = world.rng.rand() as f32 * 2.0f32 / 32767.0f32 - 1.0f32;
            let y = world.rng.rand() as f32 * 2.0f32;
            set_accel(e, [x * 10.0f32, (y / 32767.0f32 - 1.0f32) * 10.0f32, 0.0f32 * 10.0f32]);
        } else {
            let mut a = [(spawn[0].wrapping_sub(pos[0])) as f32 * K, (spawn[1].wrapping_sub(pos[1])) as f32 * K, 0.0f32];
            let len = f64::from(a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt() as f32;
            let s = 1.0f32 / len;
            a = [s * a[0] * 10.0f32, a[1] * s * 10.0f32, a[2] * s * 10.0f32];
            set_accel(e, a);
        }
        *timer = world.rng.rand() % 5000 + 3000;
        e.0[0x58] = 0;
    }
    true
}
