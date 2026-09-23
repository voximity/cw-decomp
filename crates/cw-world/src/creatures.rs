//! `cube::World::populateZoneCreatures`, `Server.exe 0x005104e0`. See
//! `analysis/notes/functions/005104e0_populateZoneCreatures.md` and `analysis/notes/porting-brief.md`.
//!
//! The behaviour tree the original attaches to every creature (`spawn+0x109c`) is recorded as
//! [`Spawn::ai`](crate::zone::Spawn::ai): [`SpawnAi::Camp`], [`SpawnAi::GroupLeader`] and
//! [`SpawnAi::GroupFollower`].

use std::f64::consts::PI;

use cw_math::MsvcRand;

use crate::fixed::to_block;
use crate::climate::div_trunc;
use crate::region::Cell;
use crate::world::World;
use crate::zone::{Spawn, SpawnAi, Static, Zone};

/// One roaming group template of `populateZoneCreatures`: `{vector<int> leaders; vector<int>
/// followers}` (24 bytes in the original, ctor `FUN_004f7540`).
pub type GroupTypes = (Vec<i32>, Vec<i32>);

/// The creature tables phase B of `populateZoneCreatures` builds for one theme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatureTables {
    /// `local_3fd`: camps get a camp fire and four props.
    pub has_props: bool,
    /// `local_420`: creature types of a camp.
    pub camp_types: Vec<i32>,
    /// `local_430`: roaming group templates, in push order.
    pub groups: Vec<GroupTypes>,
}

/// Phase A of `Server.exe 0x005104e0`: the creature theme (0..=10) of a cell, from its type
/// (`cell[6]`) and its id (`cell[8]`, reduced with an unsigned modulo). No `rand()`.
pub fn creature_theme(kind: i32, id: i32) -> i32 {
    let mut themes: Vec<i32> = Vec::new();
    if kind != 4 {
        themes.extend_from_slice(&[0, 1, 2, 3, 4, 5, 1, 7, 8]);
        if kind != 0xd && kind != 2 {
            themes.push(6);
        }
        if kind == 3 {
            themes.push(10);
        }
    } else {
        themes.push(9);
    }
    themes[(id as u32 % themes.len() as u32) as usize]
}

/// `rand() % n` with the MSVC signed remainder (`rand()` is never negative, so this is `%`).
#[inline]
fn rmod(rng: &mut MsvcRand, n: i32) -> i32 {
    rng.rand() % n
}

/// Phase B of `Server.exe 0x005104e0`: the camp types and roaming group templates of a theme.
/// Themes 1 to 4 consume `rand()`.
pub fn creature_tables(theme: i32, rng: &mut MsvcRand) -> CreatureTables {
    let mut camp = Vec::new();
    let mut groups: Vec<GroupTypes> = Vec::new();
    let mut has_props = false;
    // The Pair temporary `P`; "clear" sets both ends back to the begins.
    let mut l: Vec<i32> = Vec::new();
    let mut f: Vec<i32> = Vec::new();
    // The switch is on `theme - 1` as unsigned: theme 0 is the default case.
    match theme {
        1 => {
            has_props = true;
            camp.extend_from_slice(&[0xf, 0x10]);
            // 0x0051077d: rand() % 4
            match rmod(rng, 4) {
                0 => l.push(0x11),
                1 => l.push(0x29),
                2 => l.push(0x61),
                _ => {} // 3: the leader list stays empty
            }
            // 0x005107d6: rand() % 4
            match rmod(rng, 4) {
                0 => f.extend_from_slice(&[0x28, 0x25, 0x26, 0x27]),
                1 => f.push(0x3b),
                2 => f.push(0x29),
                _ => f.push(0x60),
            }
            groups.push((l.clone(), f.clone()));
            l.clear();
            f.clear();
            l.extend_from_slice(&[0xf, 0x10]);
            f.extend_from_slice(&[0xf, 0x10]);
            groups.push((l, f));
        }
        2 => {
            has_props = true;
            camp.extend_from_slice(&[2, 3]);
            // 0x0051099b: rand() % 3
            match rmod(rng, 3) {
                0 => l.push(0x5e),
                1 => l.push(0x4f),
                _ => l.push(0x52),
            }
            // 0x005109eb: rand() % 4 (the cases 4 -> 0x1f and 5 -> 0x20 are unreachable)
            match rmod(rng, 4) {
                0 => f.push(0x1e),
                1 => f.push(0x1a),
                2 => f.push(0x13),
                _ => f.push(0x21),
            }
            groups.push((l.clone(), f.clone()));
            l.clear();
            f.clear();
            l.extend_from_slice(&[2, 3]);
            f.extend_from_slice(&[2, 3]);
            groups.push((l, f));
        }
        3 => {
            has_props = true;
            camp.extend_from_slice(&[9, 10]);
            // 0x00510b80: rand() % 2
            match rmod(rng, 2) {
                0 => l.push(0x2f),
                _ => l.push(0x58),
            }
            // 0x00510bc4: rand() % 3
            match rmod(rng, 3) {
                0 => f.push(0x57),
                1 => f.push(0x5b),
                _ => f.push(0x21),
            }
            groups.push((l.clone(), f.clone()));
            l.clear();
            f.clear();
            l.extend_from_slice(&[9, 10]);
            f.extend_from_slice(&[9, 10]);
            groups.push((l, f));
        }
        4 => {
            has_props = true;
            camp.extend_from_slice(&[7, 8]);
            l.extend_from_slice(&[7, 8]);
            // 0x00510d51: rand() % 3
            match rmod(rng, 3) {
                0 => f.push(0x3c),
                1 => f.push(0x35),
                _ => f.push(0x3a),
            }
            groups.push((l.clone(), f.clone()));
            l.clear();
            f.clear();
            l.extend_from_slice(&[7, 8]);
            f.extend_from_slice(&[7, 8]);
            groups.push((l, f));
        }
        5 => {
            has_props = true;
            camp.extend_from_slice(&[4, 5]);
            groups.push((vec![0x58], vec![0x57, 0x37, 0x3c]));
            groups.push((vec![4, 5], vec![4, 5]));
        }
        6 => {
            camp.extend_from_slice(&[0x46, 0x45]);
            groups.push((vec![0x47], vec![0x47]));
            groups.push((vec![0x46, 0x45], vec![0x46, 0x45]));
        }
        7 => {
            camp.push(0x34);
            l.push(0x3e);
            f.push(0x3e);
            groups.push((l.clone(), f.clone()));
            l.clear();
            f.clear();
            l.push(0x3c);
            f.push(0x3c);
            groups.push((l.clone(), f.clone()));
            // The original only resets the follower list here, so the leader list keeps 0x3c.
            f.clear();
            l.push(0x34);
            f.push(0x34);
            groups.push((l, f));
        }
        8 => {
            camp.extend_from_slice(&[0x66, 0x69, 0x68]);
            groups.push((vec![0x68], vec![0x68]));
            groups.push((vec![0x66], vec![0x66]));
            groups.push((vec![0x69], vec![0x69]));
        }
        9 => {
            camp.extend_from_slice(&[0x56, 0x6a]);
            groups.push((vec![0x56], vec![0x56]));
            groups.push((vec![0x6a], vec![0x6a]));
        }
        10 => {
            camp.extend_from_slice(&[0x23, 0x5a, 0x5b]);
            groups.push((vec![0x23], vec![0x23]));
            groups.push((vec![0x5a], vec![0x5a]));
            groups.push((vec![0x5b], vec![0x5b]));
        }
        _ => {
            has_props = true;
            camp.extend_from_slice(&[0xb, 0xc]);
            groups.push((vec![0x2e], vec![0x13, 0x21, 0x1a]));
            groups.push((vec![0xb, 0xc], vec![0xb, 0xc]));
        }
    }
    CreatureTables {
        has_props,
        camp_types: camp,
        groups,
    }
}

/// `x % (uint)len` as the original indexes its vectors (unsigned DIV).
#[inline]
fn pick(r: i32, len: usize) -> usize {
    (r as u32 % len as u32) as usize
}

impl World {
    /// `populateZoneCreatures(zone, cell, points)`, `Server.exe 0x005104e0`. `points` are the
    /// candidate ground positions generateZone collected, in 16.16 fixed blocks.
    pub fn populate_zone_creatures(&mut self, zone: &mut Zone, cell: &Cell, points: &[(i64, i64, i64)]) {
        if cell.kind == 0 {
            return;
        }

        // Phases A and B: the theme and its creature tables.
        let theme = creature_theme(cell.kind, cell.id);
        let tables = creature_tables(theme, &mut self.rng);

        // Phase C: the point nearest the cell centre, if the centre lies in this zone.
        let mut best: i32 = -1;
        let cx = (cell.x / 65536) as i32;
        let cy = (cell.y / 65536) as i32;
        if div_trunc(cx, 256) == zone.x && div_trunc(cy, 256) == zone.y {
            let mut best_w = 0.0f32;
            for (i, p) in points.iter().enumerate() {
                let d = cell.norm_distance(p.0, p.1);
                let w = 1.0f32 - d;
                let w = if 0.0 < w { w * w } else { 0.0 };
                if w > best_w {
                    best_w = w;
                    best = i as i32;
                }
            }
        }

        // Phase D: every candidate point.
        const K: f32 = 1.5258789e-05; // 2^-16
        for i in 0..points.len() {
            let mut base = points[i];

            // Other points 5 to 128 blocks away (x87 FILD of the i64 difference, FSTP float,
            // then an SSE multiply; the z difference is computed the same way and unused).
            let mut neighbours: Vec<(i64, i64, i64)> = Vec::new();
            for q in points {
                let dx = (q.0 - base.0) as f32 * K;
                let dy = (q.1 - base.1) as f32 * K;
                let d2 = dy * dy + dx * dx;
                if d2 > 25.0 && 16384.0 > d2 {
                    neighbours.push(*q);
                }
            }

            let camp = if i as i32 == best {
                true
            } else {
                // 0x00511b4b: rand() % 2
                if self.rng.rand() % 2 != 0 {
                    continue;
                }
                // 0x00511b95: rand() % 2
                self.rng.rand() % 2 != 0
            };

            if camp && !tables.camp_types.is_empty() {
                if tables.has_props {
                    base = self.place_camp_props(zone, base);
                }
                self.spawn_camp(zone, cell, &tables.camp_types, base, i as i32 == best);
            } else if !tables.groups.is_empty() {
                self.spawn_group(zone, cell, &tables.groups, base, &neighbours);
            }
        }
    }

    /// The camp fire and four props of a camp (`populateZoneCreatures` from `0x00511bc8`).
    /// Returns the camp centre: the settled fire position, or `base` if the fire fits nowhere.
    fn place_camp_props(&mut self, zone: &mut Zone, mut base: (i64, i64, i64)) -> (i64, i64, i64) {
        let mut fire = Static {
            kind: 0x41,
            scale: [2.4, 2.4, 0.5],
            ..Static::NEW
        };
        // 0x00511ce4: rand() % 4
        fire.rotation = self.rng.rand() % 4;
        'search: for dx in 0..3i64 {
            for dy in 0..3i64 {
                fire.x = base.0 + (dx << 16);
                fire.y = base.1 + (dy << 16);
                fire.z = base.2;
                if self.settle_static(zone, &mut fire, true) {
                    zone.statics.push(fire.clone());
                    base = (fire.x, fire.y, fire.z);
                    break 'search;
                }
            }
        }
        // `_ftol2(229376.0)` of the double at 0x00573818: 3.5 blocks.
        let off = 229376.0f64 as i64;
        for a in (0..14i64).step_by(7) {
            for b in (0..14i64).step_by(7) {
                let mut prop = Static::NEW;
                // 0x005121fd: rand() % 4
                match self.rng.rand() % 4 {
                    1 => {
                        prop.kind = 0x10;
                        prop.scale = [1.0, 1.0, 0.5];
                    }
                    2 => {
                        prop.kind = 0xc;
                        prop.scale = [3.0, 3.0, 1.0];
                    }
                    3 => {
                        prop.kind = 0x45;
                        prop.scale = [2.0, 2.0, f32::from_bits(0x3dcccccd)];
                    }
                    _ => {
                        prop.kind = 0x42;
                        prop.scale = [4.0, 4.0, 3.0];
                    }
                }
                // 0x005122cb: rand() % 4
                prop.rotation = self.rng.rand() % 4;
                prop.x = base.0 + (a << 16) - off;
                prop.y = base.1 + (b << 16) - off;
                prop.z = base.2;
                if self.settle_static(zone, &mut prop, true) {
                    zone.statics.push(prop);
                }
            }
        }
        base
    }

    /// The 1-3 creatures of a camp, in a ring of radius 3 around `base`
    /// (`populateZoneCreatures` from `0x00512701`).
    fn spawn_camp(&mut self, zone: &mut Zone, cell: &Cell, camp_types: &[i32], base: (i64, i64, i64), is_best: bool) {
        // 0x00512707: rand(), as double * 2pi / 32767.0, then CVTPD2PS.
        let angle0 = (f64::from(self.rng.rand()) * (2.0 * PI) / 32767.0) as f32;
        // 0x0051272d: rand() % 3 + 1
        let count = self.rng.rand() % 3 + 1;
        let count_d = f64::from(count);
        for k in 0..count {
            let a = (f64::from(k * 2) * PI / count_d + f64::from(angle0)) as f32;
            let mut s = Spawn::NEW;
            s.f28 = 1;
            s.appearance.flags |= 0x1000;
            s.rotation = (f64::from(a * 180.0f32) / PI + 90.0) as f32;
            let fy = cw_math::sin(f64::from(a)) as f32 * 3.0f32 * 65536.0f32;
            let oy = fy as i64;
            let fx = cw_math::cos(f64::from(a)) as f32 * 3.0f32 * 65536.0f32;
            let ox = fx as i64;
            s.x = ox + base.0;
            s.y = oy + base.1;
            s.z = base.2;
            // 0x005128a4: rand() % (uint)campTypes.size()
            s.entity_type = camp_types[pick(self.rng.rand(), camp_types.len())];
            s.level = cell.level;
            s.b58 = cell.level_extra as u8;
            // 0x00513079: the WalkPath holds the position as built here.
            s.ai = Some(SpawnAi::Camp { path: vec![[s.x, s.y, s.z]] });
            if is_best && k == 0 {
                s.appearance.flags |= 0x200;
                s.b10e8 = 1;
            }
            self.init_appearance(&mut s);
            self.init_creature(&mut s);
            zone.spawns.push(s);
        }
    }

    /// A roaming group: a leader with a patrol path through up to three neighbouring points and
    /// 1-3 followers (`populateZoneCreatures` from `0x00512b3e`). No appearance or equipment
    /// initialisation happens here.
    fn spawn_group(
        &mut self,
        zone: &mut Zone,
        cell: &Cell,
        groups: &[GroupTypes],
        base: (i64, i64, i64),
        neighbours: &[(i64, i64, i64)],
    ) {
        // 0x00512b65: rand() % (uint)groups.size()
        let g = pick(self.rng.rand(), groups.len());
        let (leaders, followers) = &groups[g];
        if leaders.is_empty() {
            return;
        }
        let mut leader = Spawn::NEW;
        leader.f28 = 1;
        leader.appearance.flags |= 0x1000;
        // 0x00512bf1: (float)(rand() % 360)
        leader.rotation = (self.rng.rand() % 360) as f32;
        leader.x = base.0;
        leader.y = base.1;
        leader.z = base.2;
        // 0x00512c55: rand() % (uint)leaders.size()
        leader.entity_type = leaders[pick(self.rng.rand(), leaders.len())];
        leader.level = cell.level;
        leader.b58 = cell.level_extra as u8;

        // The WalkPathBehavior patrol path: the leader position, then up to three neighbours
        // raised to the first air or water block above the column's `f14`.
        let mut path: Vec<(i64, i64, i64)> = vec![base];
        if !neighbours.is_empty() {
            for _ in 0..3 {
                // 0x00512de3: rand() % (uint)neighbours.size()
                let q = neighbours[pick(self.rng.rand(), neighbours.len())];
                let bx = (q.0 / 65536) as i32;
                let by = (q.1 / 65536) as i32;
                // getColumn(bx, by, zone), `Server.exe 0x00406100`, with the zone hint.
                let in_range = (0..0x1000000).contains(&bx) && (0..0x1000000).contains(&by);
                if in_range && zone.contains(bx, by) {
                    let mut z = i64::from(zone.column(bx, by).f14) << 16;
                    loop {
                        let t = zone.block(to_block(q.0), to_block(q.1), to_block(z))[3] & 0x1f;
                        if t == 0 || t == 2 {
                            break;
                        }
                        z += 0x10000;
                    }
                    path.push((q.0, q.1, z));
                }
            }
        }
        // 0x005128e3
        leader.ai = Some(SpawnAi::GroupLeader { path: path.iter().map(|p| [p.0, p.1, p.2]).collect() });
        zone.spawns.push(leader.clone());

        if !followers.is_empty() {
            // 0x00512f92: rand() % 3 + 1
            let m = self.rng.rand() % 3 + 1;
            for _ in 0..m {
                let mut s = Spawn::NEW;
                s.x = leader.x;
                s.y = leader.y;
                s.z = leader.z;
                s.f28 = 1;
                s.appearance.flags |= 0x1000;
                // 0x00513030: rand() % (uint)followers.size()
                s.entity_type = followers[pick(self.rng.rand(), followers.len())];
                s.level = cell.level;
                s.b58 = cell.level_extra as u8;
                // 0x00512c9d: the CompanionBehavior copies the leader's `+0x48`, still 0 here
                // (the ids are assigned later by generateZone).
                s.ai = Some(SpawnAi::GroupFollower { leader: leader.id() });
                zone.spawns.push(s);
            }
        }
    }
}
