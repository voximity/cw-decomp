//! The world's pet type list (`World+0x88`, filled by the World ctor, Server.exe
//! 0x004cd507..0x004cd836) and the loot's bait item made from it (`randomConsumable`
//! 0x0052b3f0, called by `dropLoot` 0x004d2ae0 at 0x004d2f2a).

use cw_math::MsvcRand;
use cw_sim::combat::random_consumable;
use cw_world::World;
use cw_world::inventory::Item;
use cw_world::world::PET_TYPES;

/// The 45 `vector<int>::push_back` (0x004f2be0) immediates of the Server.exe ctor, in order.
/// Cube.exe's ctor (0x00593a97..0x00593dbd) pushes the same list.
const CTOR_PUSHES: [i32; 45] = [
    0x23, 0x57, 0x3c, 0x37, 0x22, 0x17, 0x16, 0x1e, 0x21, 0x62, 0x19, 0x35, 0x43, 0x66, 0x68, 0x69, 0x13, 0x28, 0x25, 0x26,
    0x27, 0x5c, 0x5d, 0x38, 0x5a, 0x5b, 0x32, 0x1b, 0x4b, 0x1a, 0x56, 0x63, 0x6a, 0x3f, 0x42, 0x40, 0x41, 0x4a, 0x24, 0x39,
    0x3a, 0x3b, 0x3d, 0x3e, 0x58,
];

#[test]
fn pet_types_match_the_world_ctor() {
    assert_eq!(PET_TYPES, CTOR_PUSHES);
}

#[test]
fn random_consumable_is_a_bait_for_a_listed_pet_type() {
    for seed in [0u32, 1, 7, 12345, 0xdead_beef] {
        let mut world = World::new(1);
        world.rng = MsvcRand::new(seed);
        let mut expected_rng = MsvcRand::new(seed);
        let r = expected_rng.rand();
        let item = random_consumable(&mut world);
        // One `rand()` and nothing else.
        assert_eq!(world.rng.rand(), expected_rng.rand());
        let expected = Item { item_type: 0x14, sub_type: PET_TYPES[(r as u32 % 45) as usize] as u8, ..Item::NEW };
        assert_eq!(item, expected, "seed {seed}");
        assert_eq!(item.level, 1);
        assert_eq!(item.rarity, 0);
    }
}
