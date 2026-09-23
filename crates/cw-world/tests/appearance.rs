//! Hand-checked cases of `Creature::initAppearance` (`Server.exe 0x0040a840`), its constructor
//! `FUN_00406970` and `FUN_004fb480`.

use cw_math::MsvcRand;
use cw_world::World;
use cw_world::appearance::{Appearance, Item};
use cw_world::zone::Spawn;

/// Little-endian bytes of a list of dwords.
fn dwords(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

#[test]
fn constructor_layout() {
    // FUN_00406970 stores, dword by dword from +0 to +0xa8.
    let want = dwords(&[
        0xffff0000, // +0x00: u16 0, hair r, g = 0xff
        0x000000ff, // +0x04: hair b = 0xff, +5 unwritten (0), flags 0
        0x3f800000, 0x3f800000, 0x3f800000, // +0x08 scale
        0xffffffff, 0xffffffff, 0xffffffff, 0xffffffff, // +0x14..+0x24 models
        0x3f8147ae, 0x3f800000, 0x3f800000, 0x3f7ae148, 0x3f800000, // +0x24..+0x34
        0x3f733333, 0x3f4ccccd, 0x3f800000, 0x3f800000, // +0x38..+0x44
        0, 0, 0, 0, 0, 0, 0, // +0x48..+0x60 pitches
        0, 0, 0xc0a00000, // +0x64 body offset
        0, 0x3f000000, 0x40a00000, // +0x70 head offset
        0x40c00000, 0, 0, // +0x7c hand offset
        0x40400000, 0x3f800000, 0xc1280000, // +0x88 foot offset
        0, 0xc1000000, 0x40000000, // +0x94 back offset
        0, 0, 0, // +0xa0 wing offset
    ]);
    assert_eq!(want.len(), Appearance::SIZE);
    assert_eq!(Appearance::NEW.to_bytes().to_vec(), want);
}

#[test]
fn item_constructor_layout() {
    let bytes = Item::NEW.to_bytes();
    assert_eq!(bytes.len(), 0x118);
    let mut want = [0u8; 0x118];
    want[0x10] = 1; // level
    assert_eq!(bytes, want);
}

fn spawn(entity_type: i32) -> Spawn {
    Spawn { entity_type, ..Spawn::NEW }
}

#[test]
fn elf_male_from_state_1() {
    // rand() from state 1: 41, 18467, 6334, 26500, 19169.
    let mut world = World::new(0);
    world.rng = MsvcRand::new(1);
    let mut s = spawn(0);
    world.init_appearance(&mut s);
    let a = &s.appearance;
    // hair = (r3, r2, r1) low bytes; variant A = 26500 % 100 = 0, B = 19169 % 100 = 69.
    assert_eq!(a.hair_color, [6334u16 as u8, 18467u16 as u8, 41]);
    assert_eq!(a.head_model, 0x4d4);
    assert_eq!(a.hair_model, 0x500 + 9);
    assert_eq!((a.hand_model, a.foot_model, a.body_model), (0x1ae, 0x1b0, 1));
    assert_eq!(a.scale.map(f32::to_bits), [0x3f75c290, 0x3f75c290, 0x400a3d71]);
    assert_eq!(a.flags, 0);
    // Untouched fields keep the constructor values.
    assert_eq!(a.head_scale.to_bits(), 0x3f8147ae);
    assert_eq!(a.tail_model, 0xffff);
}

/// Number of `rand()` calls one `init_appearance` makes for `entity_type`.
fn appearance_rand_count(entity_type: i32) -> usize {
    let mut world = World::new(0);
    world.rng = MsvcRand::new(12345);
    let mut s = spawn(entity_type);
    world.init_appearance(&mut s);
    let mut probe = MsvcRand::new(12345);
    let next = world.rng.rand();
    (0..64).find(|_| probe.rand() == next).expect("rand stream not found")
}

#[test]
fn appearance_rand_counts() {
    for t in 0..0xa0 {
        let extra = match t {
            0x06 => 4,
            0x2e => 3,
            0x3a | 0x53 | 0x54 | 0x74 | 0x76 | 0x7c | 0x8f | 0x91 | 0x92 => 1,
            0x77 => 8,
            _ => 0,
        };
        assert_eq!(appearance_rand_count(t), 5 + extra, "entity type {t:#x}");
    }
}

#[test]
fn break_cases_set_flag_0x20() {
    let mut world = World::new(0);
    for (t, flags) in [(0x2b, 0x20), (0x2c, 0), (0x11, 0x428), (0x29, 0x42b), (0x98, 0x531), (0x5c, 0x2a)] {
        let mut s = spawn(t);
        world.init_appearance(&mut s);
        assert_eq!(s.appearance.flags, flags, "entity type {t:#x}");
    }
}

#[test]
fn scale_is_divided_in_float() {
    let mut world = World::new(0);
    let mut s = spawn(0x30);
    world.init_appearance(&mut s);
    let want =
        [f32::from_bits(0x3f75c290) / 1.2f32, f32::from_bits(0x3f75c290) / 1.2, f32::from_bits(0x400a3d71) / 1.2];
    assert_eq!(s.appearance.scale.map(f32::to_bits), want.map(f32::to_bits));
    assert_eq!((s.appearance.head_model, s.appearance.foot_model, s.appearance.body_model), (0xc, 0xf, 0xd));
}

#[test]
fn creature_with_f28_6_only_draws_modifiers() {
    // f28 == 6 skips class and equipment selection: only the six final modifiers are drawn.
    let mut world = World::new(0);
    world.rng = MsvcRand::new(7);
    let mut s = Spawn { f28: 6, level: 3, ..spawn(0x84) };
    world.init_creature(&mut s);
    let mut probe = MsvcRand::new(7);
    let want: Vec<i32> = (0..6).map(|_| probe.rand()).collect();
    let eq = &s.equipment;
    let got = vec![eq[6].modifier, eq[7].modifier, eq[5].modifier, eq[2].modifier, eq[4].modifier, eq[3].modifier];
    assert_eq!(got, want);
    assert_eq!((eq[6].level, eq[7].level, eq[2].level), (3, 3, 1));
    assert_eq!(s.f30, 0);
}

#[test]
fn creature_flag_0x40_is_untouched() {
    let mut world = World::new(0);
    let mut s = spawn(0x2);
    s.appearance.flags = 0x40;
    let before = s.clone();
    let rng = world.rng;
    world.init_creature(&mut s);
    assert_eq!(s, before);
    assert_eq!(world.rng, rng);
}

#[test]
fn creature_class_pick_elf() {
    // Human-like type: class = r % 4 + 1, then per-class draws, then armour (0x20 flag clear).
    let mut world = World::new(0);
    world.rng = MsvcRand::new(1);
    let mut s = Spawn { level: 5, ..spawn(0) };
    world.init_creature(&mut s);
    // rand from state 1: 41 -> class 41 % 4 + 1 = 2 (warrior); 18467 % 2 = 1 spec;
    // 6334 % 3 = 1 -> weapon sub 7, material 2.
    assert_eq!(s.f30, 0x0102);
    let w = &s.equipment[7];
    assert_eq!((w.item_type, w.sub_type, w.material, w.level), (3, 7, 2, 5));
    // Chest: type 4, material 0x1a for sub types 6..=8.
    let c = &s.equipment[2];
    assert_eq!((c.item_type, c.material, c.level), (4, 0x1a, 5));
}
