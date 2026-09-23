//! The name helpers of the client's `cube::World` (`GameController+0x2e4`) and its
//! `cube::Speech` (`World+0x30`, the dictionary `dict_en.xml` of `data4.db`,
//! [`crate::ui::textdb::TextDb`]): the key tables the World constructor fills, the item
//! namer, the creature type names, the map's site/cell/region names, the quest objective
//! text and the crafting-station test.
//!
//! Tier B. Every string is either a key or literal from Cube.exe (address cited) or a value of
//! `dict_en.xml` looked up through those keys.
//!
//! | Original | Here |
//! |---|---|
//! | the key maps of `cube::World::World` 0x0058eb00 (0x0058ee30..0x005931ff) | [`CREATURE_TYPE_KEYS`], [`ITEM_KEYS`], [`LANDSCAPE_KEYS`], [`SITE_KEYS`], [`STATIC_KEYS`] |
//! | `World::creatureTypeKey` 0x0059aa60 (map `World+0x800104`) | [`creature_type_key`] |
//! | `World::itemKey` 0x0059fbf0 (map `World+0x800134`) | [`item_key`] |
//! | `World::materialKey` 0x0059ff60 (switch, jump table 0x005a01e0 / byte table 0x005a0248) | [`material_key`] |
//! | `World::landscapeKey` 0x005a5240 (map `World+0x800124`) | [`landscape_key`] |
//! | `World::siteKey` 0x005a64b0 via 0x0059fe70 (map `World+0x80012c`) | [`site_key`] |
//! | `Speech::form(key, form)` 0x00480e00 | [`form`] |
//! | `Speech::nameEntry(key)` 0x005a02d0 | [`name_entry`] |
//! | `World::itemName` 0x00598a50 | [`item_name`] |
//! | the creature type's `singular` (0x00480e00 over 0x0059aa60) | [`creature_singular`] |
//! | `World::regionName(x, y)` 0x005a6550 | [`region_name`] |
//! | `Speech::siteName` 0x004e5a20 (map site labels) | [`site_name`] |
//! | `Speech::cellName` 0x004e5590 (map cell labels) | [`cell_name`] |
//! | `GameController::objectiveText` 0x00477fa0 with `World::addNameVars` 0x00594c80 and `World::addSceneryVars` 0x005953a0 | [`objective_lines`], [`objective_text`] |
//! | the station tests of `BlueprintPreviewWidget::update` 0x0042f910 (jump table 0x00434770) over `Zone::getStaticCell` 0x0042f640 | [`station_ok`] |

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};

use cw_net::EntityData;
use cw_world::region::Cell;
use cw_world::zone::Static;
use cw_world::{Seeds, World};

use crate::ui::crafting::Station;
use crate::ui::names::generate_name;
use crate::ui::textdb::{Lines, NameEntry, TextDb};
use crate::ui::tooltip::join_words;

// ---------------------------------------------------------------------------------------
// The World constructor's key maps (0x0058eb00)
// ---------------------------------------------------------------------------------------

/// `World+0x800104` (`std::map<int, wstring>`, filled at 0x0058ee30..0x005901b2 with
/// `operator[]` 0x004e2df0 then `assign` 0x0040f7a0): creature type (`entity+0x54`) to its
/// dictionary key. Two keys are assigned twice with the same value (0x3e `Mosquito`, 0x24
/// `Porcupine`).
pub const CREATURE_TYPE_KEYS: &[(i32, &str)] = &[
    (0x00, "ElfMale"),
    (0x01, "ElfFemale"),
    (0x02, "HumanMale"),
    (0x03, "HumanFemale"),
    (0x04, "GoblinMale"),
    (0x05, "GoblinFemale"),
    (0x06, "Bullterrier"),
    (0x07, "LizardmanMale"),
    (0x08, "LizardmanFemale"),
    (0x0d, "FrogmanMale"),
    (0x0e, "FrogmanFemale"),
    (0x60, "Zombie"),
    (0x2c, "Bandit"),
    (0x12, "OldMan"),
    (0x2b, "Wizard"),
    (0x6c, "Troll"),
    (0x6e, "HellDemon"),
    (0x6f, "Golem"),
    (0x70, "EmberGolem"),
    (0x71, "SnowGolem"),
    (0x5e, "Werewolf"),
    (0x6d, "DarkTroll"),
    (0x72, "Yeti"),
    (0x2e, "Ogre"),
    (0x2f, "Rockling"),
    (0x61, "Vampire"),
    (0x56, "Spitter"),
    (0x4c, "SpikeCreature"),
    (0x4d, "Anubis"),
    (0x4e, "Horus"),
    (0x4f, "Jester"),
    (0x50, "Spectrino"),
    (0x51, "Djinn"),
    (0x52, "Minotaur"),
    (0x55, "Imp"),
    (0x65, "Dragon"),
    (0x75, "Lich"),
    (0x09, "DwarfMale"),
    (0x0a, "DwarfFemale"),
    (0x35, "Hornet"),
    (0x36, "InsectGuard"),
    (0x7f, "Ginseng"),
    (0x3c, "Fly"),
    (0x3d, "Midge"),
    (0x3e, "Mosquito"),
    (0x39, "Seagull"),
    (0x46, "RadishCreature"),
    (0x45, "PlantCreature"),
    (0x47, "Onionling"),
    (0x48, "DesertOnionling"),
    (0x49, "Devourer"),
    (0x6a, "Crab"),
    (0x6b, "SeaCrab"),
    (0x66, "BarkBeetle"),
    (0x67, "FireBeetle"),
    (0x68, "SnoutBeetle"),
    (0x69, "LemonBeetle"),
    (0x92, "LemonFish"),
    (0x91, "SapphireFish"),
    (0x96, "Shark"),
    (0x98, "LanternFish"),
    (0x99, "MawFish"),
    (0x9a, "Piranha"),
    (0x9b, "Blowfish"),
    (0x93, "Seahorse"),
    (0x44, "Frog"),
    (0x57, "Mole"),
    (0x58, "Biter"),
    (0x5a, "Squirrel"),
    (0x5b, "Raccoon"),
    (0x4a, "Duckbill"),
    (0x4b, "Crocodile"),
    (0x5c, "Owl"),
    (0x5d, "Penguin"),
    (0x37, "Crow"),
    (0x3a, "Parrot"),
    (0x77, "Saurian"),
    (0x76, "RuneGiant"),
    (0x73, "Cyclops"),
    (0x74, "Mammoth"),
    (0x2d, "Witch"),
    (0x33, "Gnobold"),
    (0x34, "Insectoid"),
    (0x30, "Gnoll"),
    (0x31, "PolarGnoll"),
    (0x94, "Mermaid"),
    (0x95, "Merman"),
    (0x3b, "Bat"),
    (0x0b, "OrcMale"),
    (0x0c, "OrcFemale"),
    (0x25, "GreenSlime"),
    (0x26, "PinkSlime"),
    (0x27, "YellowSlime"),
    (0x28, "BlueSlime"),
    (0x29, "Frightener"),
    (0x2a, "SandHorror"),
    (0x78, "Bush"),
    (0x79, "SnowBush"),
    (0x7a, "SnowBerryBush"),
    (0x82, "ThornTree"),
    (0x80, "Cactus"),
    (0x8e, "Dummy"),
    (0x8d, "Aim"),
    (0x7b, "CottonPlant"),
    (0x7c, "Scrub"),
    (0x7d, "CobwebScrub"),
    (0x7e, "FireScrub"),
    (0x90, "Bomb"),
    (0x0f, "UndeadMale"),
    (0x10, "UndeadFemale"),
    (0x11, "Skeleton"),
    (0x3f, "PlainRunner"),
    (0x40, "LeafRunner"),
    (0x41, "SnowRunner"),
    (0x42, "DesertRunner"),
    (0x43, "Peacock"),
    (0x23, "Bunny"),
    (0x24, "Porcupine"),
    (0x13, "Collie"),
    (0x14, "ShepherdDog"),
    (0x16, "Alpaca"),
    (0x17, "BrownAlpaca"),
    (0x15, "SkullBull"),
    (0x18, "Egg"),
    (0x19, "Turtle"),
    (0x1a, "Terrier"),
    (0x1b, "ScottishTerrier"),
    (0x1d, "Panther"),
    (0x1e, "Cat"),
    (0x1f, "BrownCat"),
    (0x20, "WhiteCat"),
    (0x1c, "Wolf"),
    (0x21, "Pig"),
    (0x22, "Sheep"),
    (0x62, "Horse"),
    (0x64, "Cow"),
    (0x63, "Camel"),
    (0x32, "Monkey"),
    (0x38, "Chicken"),
    (0x3e, "Mosquito"),
    (0x24, "Porcupine"),
    (0x8f, "Vase"),
    (0x84, "IronDeposit"),
    (0x83, "GoldDeposit"),
    (0x87, "EmeraldDeposit"),
    (0x88, "SapphireDeposit"),
    (0x89, "RubyDeposit"),
    (0x8a, "DiamondDeposit"),
    (0x85, "SilverDeposit"),
    (0x86, "SandstoneDeposit"),
    (0x8b, "IceCrystalDeposit"),
    (0x8c, "Scarecrow"),
    (0x53, "NomadMale"),
    (0x54, "NomadFemale"),
    (0x97, "Bumblebee"),
    (0x59, "Koala"),
];

/// `World+0x800134` (`std::map<pair<int,int>, wstring>`, operator[] 0x005943b0, filled at
/// 0x005901ce..0x00590e..): `(item type, sub type)` to its dictionary key, in insertion order
/// (a later entry for the same pair overwrites). The Bait loop (0x005906eb: `(0x14, 0..0x9b)`
/// all `Bait`) sits between the `ManaCube` and `BlueJelly` entries and is applied by
/// [`item_key`]; the pet-food entries after it overwrite it. `0x0071b3d0` = `Axe`,
/// `0x0071b3d8` = `Bow`, `0x00700a30` = `Pet`, `0x0071bad8` = `Log`, `0x0071bb5c` = `Orb`,
/// `0x0071bcf8` = `Key`.
pub const ITEM_KEYS: &[((i32, i32), &str)] = &[
    ((6, 0), "Boots"),
    ((5, 0), "Gloves"),
    ((4, 0), "ChestArmor"),
    ((7, 0), "ShoulderArmor"),
    ((0x13, 0), "Pet"),
    ((8, 0), "Amulet"),
    ((9, 0), "Ring"),
    ((2, 0), "Formula"),
    ((3, 0), "Sword"),
    ((3, 3), "Dagger"),
    ((3, 4), "Fist"),
    ((3, 2), "Mace"),
    ((3, 1), "Axe"),
    ((3, 6), "Bow"),
    ((3, 7), "Crossbow"),
    ((3, 8), "Boomerang"),
    ((3, 0xe), "Arrows"),
    ((3, 9), "Arrow"),
    ((3, 0x14), "Torch"),
    ((3, 0xa), "Staff"),
    ((3, 0xb), "Wand"),
    ((3, 0xc), "Bracelet"),
    ((3, 0xd), "Shield"),
    ((3, 0xf), "Greatsword"),
    ((3, 0x10), "Greataxe"),
    ((3, 0x11), "Greatmace"),
    ((3, 5), "Longsword"),
    ((0xc, 0), "Coin"),
    ((0xd, 0), "PlatinumCoin"),
    ((0x12, 0), "Candle"),
    ((0x12, 1), "Candle"),
    ((0x17, 0), "HangGlider"),
    ((0x17, 1), "Boat"),
    ((0x18, 0), "Lamp"),
    ((0x19, 0), "ManaCube"),
    // (0x14, 0..0x9c) "Bait" here (see `item_key`).
    ((0x14, 0x28), "BlueJelly"),
    ((0x14, 0x25), "GreenJelly"),
    ((0x14, 0x26), "PinkJelly"),
    ((0x14, 0x27), "YellowJelly"),
    ((0x14, 0x23), "Carrot"),
    ((0x14, 0x21), "PumpkinMash"),
    ((0x14, 0x1e), "Candy"),
    ((0x14, 0x5c), "Lollipop"),
    ((0x14, 0x5d), "Softice"),
    ((0x14, 0x57), "ChocolateDonut"),
    ((0x14, 0x22), "CottonCandy"),
    ((0x14, 0x35), "Popcorn"),
    ((0x14, 0x38), "CerealBar"),
    ((0x14, 0x5a), "StrawberryCake"),
    ((0x14, 0x5b), "ChocolateCake"),
    ((0x14, 0x17), "ChocolateCupcake"),
    ((0x14, 0x16), "VanillaCupcake"),
    ((0x14, 0x32), "BananaSplit"),
    ((0x14, 0x1b), "Croissant"),
    ((0x14, 0x66), "Bread"),
    ((0x14, 0x68), "Lolly"),
    ((0x14, 0x69), "LemonTart"),
    ((0x14, 0x43), "ChocolateCookie"),
    ((0x14, 0x13), "BubbleGum"),
    ((0x14, 0x37), "LicoriceCandy"),
    ((0x14, 0x19), "CinnamonRole"),
    ((0x14, 0x4b), "AppleRing"),
    ((0x14, 0x1a), "Waffle"),
    ((0x14, 0x56), "WaterIce"),
    ((0x14, 0x63), "DateCookie"),
    ((0x14, 0x62), "CandiedApple"),
    ((0x14, 0x6a), "StrawberryCocktail"),
    ((0x14, 0x3f), "MilkChocolateBar"),
    ((0x14, 0x42), "CaramelChocolateBar"),
    ((0x14, 0x40), "MintChocolateBar"),
    ((0x14, 0x41), "WhiteChocolateBar"),
    ((0x14, 0x4a), "SugarCandy"),
    ((0x14, 0x24), "BlackberryMarmelade"),
    ((0x14, 0x39), "SaltedCaramel"),
    ((0x14, 0x3a), "GingerTartlet"),
    ((0x14, 0x3b), "MangoJuice"),
    ((0x14, 0x3c), "FruitBasket"),
    ((0x14, 0x3d), "MelonIceCream"),
    ((0x14, 0x3e), "BloodOrangeJuice"),
    ((0x14, 0x58), "Pancakes"),
    ((0x14, 0x67), "Curry"),
    ((0x14, 0x97), "BiscuitRole"),
    ((1, 1), "LifePotion"),
    ((1, 2), "CactusPotion"),
    ((1, 3), "ManaPotion"),
    ((1, 4), "GinsengSoup"),
    ((1, 5), "SnowBerryMash"),
    ((1, 6), "MushroomSpit"),
    ((1, 8), "PineappleSlice"),
    ((1, 9), "PumpkinMuffin"),
    ((1, 0), "Cookie"),
    ((1, 7), "Bomb"),
    ((0xb, 0), "Nugget"),
    ((0xb, 1), "Log"),
    ((0xb, 2), "Feather"),
    ((0xb, 3), "Horn"),
    ((0xb, 4), "Claw"),
    ((0xb, 5), "Fiber"),
    ((0xb, 7), "Hair"),
    ((0xb, 8), "Crystal"),
    ((0xb, 9), "Yarn"),
    ((0xb, 6), "Cobweb"),
    ((0xb, 0xa), "Cube"),
    ((0xb, 0xb), "Capsule"),
    ((0xb, 0xd), "Orb"),
    ((0xb, 0xc), "Flask"),
    ((0xb, 0xe), "Spirit"),
    ((0xb, 0xf), "Mushroom"),
    ((0xb, 0x13), "ShimmerMushroom"),
    ((0xb, 0x14), "GinsengRoot"),
    ((0xb, 0x10), "Pumpkin"),
    ((0xb, 0x11), "Pineapple"),
    ((0xb, 0x16), "Heartflower"),
    ((0xb, 0x17), "PricklyPear"),
    ((0xb, 0x18), "FrozenHeartflower"),
    ((0xb, 0x19), "Soulflower"),
    ((0xb, 0x12), "RadishSlice"),
    ((0xb, 0x15), "OnionSlice"),
    ((0xb, 0x1a), "WaterFlask"),
    ((0xb, 0x1b), "SnowBerry"),
    ((0xf, 0), "Beak"),
    ((0x15, 0), "Amulet1"),
    ((0x15, 1), "Amulet2"),
    ((0x15, 2), "JewelCase"),
    ((0x15, 3), "Key"),
    ((0x15, 4), "Medicine"),
    ((0x15, 5), "Antivenom"),
    ((0x15, 6), "BandAid"),
    ((0x15, 7), "Crutch"),
    ((0x15, 8), "Bandage"),
    ((0x15, 9), "Salve"),
    ((0xe, 0), "Leftovers"),
    ((0x10, 0), "Painting"),
];

/// `World+0x80010c` (`std::map<int, wstring>`, 0x00590e..): static kind (`Static+0`) to its
/// dictionary key (`0x0071be5c` = `Bed`). Not read by the helpers here; kept for the
/// examine texts.
pub const STATIC_KEYS: &[(i32, &str)] = &[
    (0, "Statue"),
    (1, "Door"),
    (2, "BigDoor"),
    (3, "Window"),
    (4, "CastleWindow"),
    (5, "Gate"),
    (6, "FireTrap"),
    (7, "SpikeTrap"),
    (8, "StompTrap"),
    (9, "Lever"),
    (0xa, "Chest"),
    (0xc, "Table"),
    (0xe, "Table"),
    (0xd, "Table"),
    (0x10, "Stool"),
    (0xf, "Stool"),
    (0x11, "Stool"),
    (0x12, "Bench"),
    (0x13, "Bed"),
    (0x14, "BedTable"),
    (0x15, "MarketStand1"),
    (0x16, "MarketStand2"),
    (0x17, "MarketStand3"),
    (0x18, "Barrel"),
    (0x19, "Crate"),
    (0x1a, "OpenCrate"),
    (0x1b, "Sack"),
    (0x1c, "Shelter"),
    (0x1d, "Cupboard"),
    (0x1e, "Desktop"),
    (0x1f, "Counter"),
    (0x20, "Shelf1"),
    (0x21, "Shelf2"),
    (0x22, "Shelf3"),
    (0x2c, "Corpse"),
    (0x2d, "RuneStone"),
    (0x2e, "Artifact"),
    (0x2f, "FlowerBox1"),
    (0x30, "FlowerBox2"),
    (0x31, "FlowerBox3"),
    (0x32, "StreetLight"),
    (0x33, "FireStreetLight"),
    (0x34, "Fence1"),
    (0x35, "Fence2"),
    (0x36, "Fence3"),
    (0x37, "Fence4"),
    (0x38, "Vase1"),
    (0x39, "Vase2"),
    (0x3a, "Vase3"),
    (0x3b, "Vase4"),
    (0x3c, "Vase5"),
    (0x3d, "Vase6"),
    (0x3e, "Vase7"),
    (0x3f, "Vase8"),
    (0x40, "Vase9"),
    (0x41, "Campfire"),
    (0x42, "Tent"),
    (0x43, "BeachUmbrella"),
    (0x44, "BeachTowel"),
    (0x45, "SleepingMat"),
    (0x47, "Furnace"),
    (0x48, "Anvil"),
    (0x49, "SpinningWheel"),
    (0x4a, "Loom"),
    (0x4b, "SawBench"),
    (0x4c, "Workbench"),
    (0x4d, "CustomizationBench"),
];

/// The kinds of the landscape loop 0x00591a..: `(kind, key)` for sub types `0..10` each
/// (`World+0x800124`).
const LANDSCAPE_LOOP: [(i32, &str); 11] = [
    (1, "Village"),
    (2, "Mountain"),
    (3, "Forest"),
    (4, "Lake"),
    (6, "Canyon"),
    (0xb, "Rock"),
    (0xc, "Tree"),
    (7, "Valley"),
    (8, "Crater"),
    (0xd, "Peak"),
    (0xf, "Island"),
];

/// `World+0x800124` after the loop (0x00591c..): the fixed `(kind, variant)` entries.
pub const LANDSCAPE_KEYS: &[((i32, i32), &str)] = &[
    ((5, 0), "Ruins"),
    ((5, 1), "Ruins"),
    ((5, 2), "Ruins"),
    ((5, 3), "Ruins"),
    ((5, 4), "Gravesite"),
    ((0xe, 0), "Castle"),
    ((0xe, 1), "Ruins"),
    ((0xe, 2), "Catacombs"),
    ((0xe, 3), "Palace"),
    ((0xe, 4), "Temple"),
    ((0xe, 5), "Pyramid"),
    ((9, 0), "Cave"),
    ((0xa, 0), "Portal"),
];

/// `World+0x80012c` (0x005930ab..0x005931ff): the zone site records' `(kind, sub)` keys.
pub const SITE_KEYS: &[((i32, i32), &str)] = &[
    ((1, 1), "Trade Quarter"),
    ((1, 2), "Crafting Quarter"),
    ((1, 3), "Class Quarter"),
    ((1, 4), "Pet Quarter"),
    ((4, 0), "Portal"),
    ((5, 0), "Palace"),
];

/// 0x0059aa60: the key of creature type `t`, `""` (0x006fccac) when the map has none.
pub fn creature_type_key(t: i32) -> &'static str {
    CREATURE_TYPE_KEYS.iter().rev().find(|(k, _)| *k == t).map_or("", |(_, v)| v)
}

/// 0x0059fbf0: the key of `(item type, sub type)` (both zero-extended bytes at the call
/// sites), `""` when missing.
pub fn item_key(item_type: i32, sub: i32) -> &'static str {
    if let Some((_, v)) = ITEM_KEYS.iter().rev().find(|(k, _)| *k == (item_type, sub)) {
        return v;
    }
    // 0x005906eb: `for i in 0..0x9c: map[(0x14, i)] = "Bait"`.
    if item_type == 0x14 && (0..0x9c).contains(&sub) {
        return "Bait";
    }
    ""
}

/// 0x0059ff60: the key of material `m` (`Item+0xd`, zero-extended; `m - 1` compared
/// unsigned against 0x82), `"Material"` for every other value.
pub fn material_key(m: i32) -> &'static str {
    match m {
        1 => "MaterialIron",
        2 => "MaterialWood",
        5 => "MaterialObsidian",
        7 => "MaterialBone",
        10 => "MaterialCopper",
        11 => "MaterialGold",
        12 => "MaterialSilver",
        13 => "MaterialEmerald",
        14 => "MaterialSapphire",
        15 => "MaterialRuby",
        16 => "MaterialDiamond",
        17 => "MaterialSandstone",
        18 => "MaterialSaurian",
        19 => "MaterialParrot",
        20 => "MaterialMammoth",
        21 => "MaterialPlant",
        22 | 130 => "MaterialIce",
        23 => "MaterialLight",
        24 => "MaterialGlass",
        25 => "MaterialSilk",
        26 => "MaterialLinen",
        27 => "MaterialCotton",
        128 => "MaterialFire",
        129 => "MaterialUnholy",
        131 => "MaterialWind",
        _ => "Material",
    }
}

/// 0x005a5240: the landscape key of a cell `(kind +0x18, variant +0x1c)`, `""` when missing.
pub fn landscape_key(kind: i32, variant: i32) -> &'static str {
    if let Some((_, v)) = LANDSCAPE_KEYS.iter().find(|(k, _)| *k == (kind, variant)) {
        return v;
    }
    if (0..10).contains(&variant) {
        if let Some((_, v)) = LANDSCAPE_LOOP.iter().find(|(k, _)| *k == kind) {
            return v;
        }
    }
    ""
}

/// 0x005a64b0 (through 0x0059fe70): the key of a zone site record `(kind +0, sub +1)`.
pub fn site_key(kind: i32, sub: i32) -> &'static str {
    SITE_KEYS.iter().find(|(k, _)| *k == (kind, sub)).map_or("", |(_, v)| v)
}

// ---------------------------------------------------------------------------------------
// Speech lookups
// ---------------------------------------------------------------------------------------

/// `Speech::nameEntry(key)` 0x005a02d0: the `name` entry of `key`, `None` for the static empty
/// entry (0x0076b7f4) the original returns when the key is missing.
pub fn name_entry<'a>(db: &'a TextDb, key: &str) -> Option<&'a NameEntry> {
    db.names.get(key)
}

/// `Speech::form(key, form)` 0x00480e00: `names[key].forms[form]`, `""` when either is missing.
pub fn form(db: &TextDb, key: &str, form: &str) -> String {
    db.names.get(key).and_then(|e| e.forms.get(form)).cloned().unwrap_or_default()
}

/// `std::wstring::replace(find(c), 1, with)` when `c` occurs (the `find` 0x004d9950 /
/// `replace` 0x00486d00 pairs of the namers). Returns whether it replaced.
fn replace_first(s: &mut String, c: char, with: &str) -> bool {
    match s.find(c) {
        Some(p) => {
            s.replace_range(p..p + c.len_utf8(), with);
            true
        }
        None => false,
    }
}

/// The adjective key of a rarity and modifier (0x00599008..0x00599b..): `item:<adj>` from
/// the table of rarity `Item+0xc` (0..4) at `modifier % 10` (unsigned).
pub fn adjective_key(rarity: u8, modifier: u32) -> Option<&'static str> {
    const T: [[&str; 10]; 5] = [
        [
            "item:shabby", "item:plain", "item:battered", "item:artless", "item:unwieldy", "item:used", "item:dusty",
            "item:scratched", "item:worn", "item:common",
        ],
        [
            "item:adjusted", "item:balanced", "item:battletested", "item:good", "item:handmade", "item:fair",
            "item:neat", "item:clean", "item:undamaged", "item:flawless",
        ],
        [
            "item:exceptional", "item:polished", "item:extraordinary", "item:exquisite", "item:superb", "item:unique",
            "item:handsome", "item:grand", "item:magic", "item:decorated",
        ],
        [
            "item:exceptional2", "item:polished2", "item:extraordinary2", "item:exquisite2", "item:superb2",
            "item:unique2", "item:handsome2", "item:grand2", "item:magic2", "item:decorated2",
        ],
        [
            "item:brilliant", "item:shining", "item:magnificent", "item:sublime", "item:pompous", "item:glorious",
            "item:splendid", "item:famous", "item:legendary", "item:fabulous",
        ],
    ];
    T.get(rarity as usize).map(|t| t[(modifier % 10) as usize])
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

/// `World::itemName(out, item)` 0x00598a50 on the raw 0x118-byte item. Without a dictionary
/// every dictionary text reads `""` (as the original's lookups of missing keys do).
///
/// * Type 2 (formula): `"Formula: "` + the name of a copy whose type is
///   `Item+8` and whose `+8..+0xc` is zeroed.
/// * Type 0x13 (pet): `"Pet: "` + the creature type *key* of the sub type (0x0059aa60, not
///   its `singular`).
/// * Type 0x15: `generateName(Item+8, Item+4)` + `"'s "` (0x0071d560) + the `singular`.
/// * Types 1, 0x12, 0xe: the `singular` of `itemKey(type, sub)`.
/// * Others: the material's `item` form (`"Iron @"`); without an `@` the `singular`, else the
///   `@` becomes the type's `singular`. With a modifier (`Item+4 != 0`), the adjective of
///   rarity `Item+0xc` (0..4) at `modifier % 10` wraps it: the first tag of the type's entry
///   (attribute order) that the adjective has a form for is used (its `@` replaced by the
///   name so far, then a `#` replaced by `generateName(modifier, modifier * 7 % 11)`, all
///   unsigned); when no tag matched, or the matched form had no `@`, the adjective's first
///   form in key order is applied the same way. (The rare-attribute histogram the original
///   builds at 0x00598c9b over `Item+0x17` is never read.)
pub fn item_name(db: Option<&TextDb>, item: &[u8]) -> String {
    let empty = TextDb::default();
    let db = db.unwrap_or(&empty);
    let t = i32::from(item[0]);
    let sub = i32::from(item[1]);
    match t {
        2 => {
            // 0x00598a9c: the copy with `type = Item+8`, `+8 = 0`.
            let mut copy = item[..cw_net::entity::ITEM_SIZE.min(item.len())].to_vec();
            copy[0] = copy[8];
            copy[8..12].fill(0);
            format!("Formula: {}", item_name(Some(db), &copy))
        }
        0x13 => format!("Pet: {}", creature_type_key(sub)),
        0x15 => {
            let singular = form(db, item_key(t, sub), "singular");
            let owner = generate_name(u32_at(item, 8), u32_at(item, 4) as i32);
            format!("{owner}'s {singular}")
        }
        1 | 0x12 | 0xe => form(db, item_key(t, sub), "singular"),
        _ => {
            let mut name = form(db, material_key(i32::from(item[0xd])), "item");
            if name.find('@').is_none() {
                return form(db, item_key(t, sub), "singular");
            }
            let entry = name_entry(db, item_key(t, sub));
            let singular = entry.and_then(|e| e.forms.get("singular")).cloned().unwrap_or_default();
            replace_first(&mut name, '@', &singular);
            let modifier = u32_at(item, 4);
            if modifier == 0 {
                return name;
            }
            let adj: BTreeMap<String, String> = adjective_key(item[0xc], modifier)
                .and_then(|k| name_entry(db, k))
                .map(|e| e.forms.clone())
                .unwrap_or_default();
            let hash_name = || generate_name(modifier, (modifier.wrapping_mul(7) % 11) as i32);
            let apply = |name: &mut String, form: &str| -> bool {
                let mut s = form.to_string();
                let replaced = replace_first(&mut s, '@', name);
                if replaced {
                    *name = s;
                }
                replace_first(name, '#', &hash_name());
                replaced
            };
            let tags: &[String] = entry.map_or(&[], |e| &e.tags);
            for tag in tags {
                if let Some(f) = adj.get(tag) {
                    if apply(&mut name, f) {
                        return name;
                    }
                    break;
                }
            }
            // 0x00599d89: the adjective's first form.
            if let Some((_, f)) = adj.iter().next() {
                apply(&mut name, f);
            }
            name
        }
    }
}

/// The `singular` of creature type `t` (0x00480e00 over 0x0059aa60): the nameplate `info`
/// text and the pet frame's class line.
pub fn creature_singular(db: Option<&TextDb>, t: i32) -> String {
    db.map_or_else(String::new, |db| form(db, creature_type_key(t), "singular"))
}

// ---------------------------------------------------------------------------------------
// Map names
// ---------------------------------------------------------------------------------------

/// The two syllable tables of 0x005a6550 (static `std::wstring` arrays 0x0076d9d8 and
/// 0x0076dbc0, 20 entries each, built on first use from the literals 0x0071d15c.. and
/// 0x0071cc2c..).
const REGION_PREFIX: [&str; 20] = [
    "Hy", "Ty", "Tri", "Ge", "Go", "Lu", "Der", "Ter", "Sel", "Ko", "Am", "Ka", "Kel", "Ko", "Sa", "Mo", "Ru", "Al", "Ta",
    "Har",
];
const REGION_SUFFIX: [&str; 20] = [
    "ra", "mala", "ura", "el", "al", "mer", "kun", "la", "mur", "gu", "la", "go", "sel", "rion", "remo", "antis", "uro",
    "lic", "reon", "tuma",
];

/// `World::regionName(x, y)` 0x005a6550 at block `(x, y)`: `""` when `x > 0xffffff`
/// (unsigned) or `y >= 0x1000000` (signed); else the warped position (`warpPosition`
/// 0x0059..., `cvttsd2si` per axis, 0x0058d6c0) `(wx, wy)` picks
/// `prefix[(wy * 3 + seed(0x80028c) + wx) % 20] + suffix[(wx * 3 + seed(0x800290) + wy) % 20]`
/// (signed `idiv`; the seeds are below 100000 and the warped position below 1024, so the
/// operands are never negative).
pub fn region_name(seeds: &Seeds, x: i32, y: i32) -> String {
    if (x as u32) > 0xff_ffff || y >= 0x100_0000 {
        return String::new();
    }
    let w = cw_world::warp::warp_position(seeds, x, y);
    let (wx, wy) = (w[0] as i32, w[1] as i32);
    let a = wy.wrapping_mul(3).wrapping_add(seeds.at(0x80028c)).wrapping_add(wx);
    let b = wx.wrapping_mul(3).wrapping_add(seeds.at(0x800290)).wrapping_add(wy);
    // Performance note for a future optimiser: warp_position is two noise pairs per label.
    let p = REGION_PREFIX[(a % 20).rem_euclid(20) as usize];
    let s = REGION_SUFFIX[(b % 20).rem_euclid(20) as usize];
    format!("{p}{s}")
}

/// `Speech::siteName(out, world, site, x, y)` 0x004e5a20 for the zone site record `site`
/// (`ZoneTile+0x10`: kind, sub, seed at `+4`) at block `(x, y)` (the zone centre, `zx * 256
/// + 128`): the `singular` of [`site_key`], `""` when empty; its `@` becomes
/// `generateName(seed, -1)` when the seed and kind are set, else [`region_name`].
pub fn site_name(db: Option<&TextDb>, seeds: Option<&Seeds>, site: &cw_world::region::ZoneRecord, x: i32, y: i32) -> String {
    let Some(db) = db else { return String::new() };
    let key = site_key(i32::from(site.kind), i32::from(site.sub));
    if key.is_empty() {
        return String::new();
    }
    let mut s = form(db, key, "singular");
    if s.is_empty() {
        return String::new();
    }
    if s.find('@').is_some() {
        let with = if site.seed == 0 || site.kind == 0 {
            seeds.map_or_else(String::new, |sd| region_name(sd, x, y))
        } else {
            generate_name(site.seed as u32, -1)
        };
        replace_first(&mut s, '@', &with);
    }
    s
}

/// `Speech::cellName(out, world, cell)` 0x004e5590: the `singular` of
/// [`landscape_key`]`(cell+0x18, cell+0x1c)`; its `@` becomes `generateName(cell+0x20, -1)`.
pub fn cell_name(db: Option<&TextDb>, kind: i32, variant: i32, id: i32) -> String {
    let Some(db) = db else { return String::new() };
    let mut s = form(db, landscape_key(kind, variant), "singular");
    if s.is_empty() {
        return s;
    }
    replace_first(&mut s, '@', &generate_name(id as u32, -1));
    s
}

// ---------------------------------------------------------------------------------------
// The quest objective (0x00477fa0)
// ---------------------------------------------------------------------------------------

/// The cell fields the objective text reads (`cube::Cell`, 0x68 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ObjectiveCell {
    /// `+0x18`, `+0x1c`, `+0x20`: landscape kind, variant and id (the scenery names).
    pub kind: i32,
    pub variant: i32,
    pub id: i32,
    /// `+0x30`: the `@name` seed.
    pub f30: i32,
    /// `+0x34`: the mission type (the `objective:` key).
    pub mission: i32,
    /// `+0x38`: the creature type (the `@name` race and the `creature` names).
    pub creature: i32,
}

impl ObjectiveCell {
    /// From a world cell.
    pub fn from_cell(c: &Cell) -> Self {
        ObjectiveCell {
            kind: c.kind,
            variant: c.variant,
            id: c.id,
            f30: c.mission.f30,
            mission: c.mission.f34,
            creature: c.mission.f38,
        }
    }
}

/// `World::addNameVars(prefix, type, ctx)` 0x00594c80 / `addSceneryVars` 0x005953a0: for
/// the entry of `key`, every tag `t` becomes the condition `#<prefix>_<t>` and every form
/// `f = v` the variable `@<prefix>_<f>` (a later assignment overwrites); `at_name` replaces
/// an `@` in each value (the scenery's `generateName(cell+0x20, -1)`).
pub(crate) fn add_entry_vars(
    db: &TextDb,
    key: &str,
    prefix: &str,
    at_name: Option<&str>,
    tags: &mut BTreeSet<String>,
    vars: &mut BTreeMap<String, String>,
) {
    let Some(e) = name_entry(db, key) else { return };
    for t in &e.tags {
        tags.insert(format!("#{prefix}_{t}"));
    }
    for (f, v) in &e.forms {
        let mut v = v.clone();
        if let Some(n) = at_name {
            replace_first(&mut v, '@', n);
        }
        vars.insert(format!("@{prefix}_{f}"), v);
    }
}

/// The `objective:` key of a mission type (0x00478135 switch on `cell+0x34 - 1`); types 6
/// and above 13 render nothing.
pub fn objective_key(mission: i32) -> Option<&'static str> {
    Some(match mission {
        1 => "objective:monster",
        2 => "objective:rareboss",
        3 => "objective:villagemonster",
        4 => "objective:villagerareboss",
        5 => "objective:dungeon",
        7 => "objective:invasion",
        8 => "objective:sceneryinvasion",
        9 => "objective:villageinvasion",
        10 => "objective:caveinvasion",
        11 => "objective:riverinvasion",
        12 => "objective:gang",
        13 => "objective:gangboss",
        _ => return None,
    })
}

/// The rendered lines of 0x00477fa0: `@name` = `generateName(cell+0x30, cell+0x38)`, the
/// `creature` names of type `cell+0x38` (0x00594c80), the `scenery` names of the cell's
/// landscape (0x005953a0), then `Speech::render` 0x004e4a20 of [`objective_key`].
pub fn objective_lines(db: &TextDb, cell: &ObjectiveCell) -> Lines {
    let mut tags = BTreeSet::new();
    let mut vars = BTreeMap::new();
    vars.insert("@name".to_string(), generate_name(cell.f30 as u32, cell.creature));
    add_entry_vars(db, creature_type_key(cell.creature), "creature", None, &mut tags, &mut vars);
    let scenery_name = generate_name(cell.id as u32, -1);
    add_entry_vars(db, landscape_key(cell.kind, cell.variant), "scenery", Some(&scenery_name), &mut tags, &mut vars);
    let mut lines = Lines::new();
    if let Some(k) = objective_key(cell.mission) {
        db.render(k, &tags, &vars, &mut lines);
    }
    lines
}

/// The objective text of 0x00477fa0: the words of the first rendered line joined by
/// [`join_words`] (the other lines are dropped).
pub fn objective_text(db: Option<&TextDb>, cell: &ObjectiveCell) -> String {
    let Some(db) = db else { return String::new() };
    let lines = objective_lines(db, cell);
    match lines.first() {
        Some(l) => join_words(&l.iter().map(|w| w.text.as_str()).collect::<Vec<_>>()),
        None => String::new(),
    }
}

// ---------------------------------------------------------------------------------------
// The crafting stations (0x0042f910 / 0x0042f640)
// ---------------------------------------------------------------------------------------

fn i64_at(e: &EntityData, o: usize) -> i64 {
    i64::from_le_bytes(e.0[o..o + 8].try_into().unwrap())
}

fn f32_at(e: &EntityData, o: usize) -> f32 {
    f32::from_le_bytes(e.0[o..o + 4].try_into().unwrap())
}

/// `World::getBlockFixed` 0x0042f860 (`Server.exe 0x00406050`): per axis a value whose high
/// dword is negative, or zero with the low dword's top bit set, first loses 0x10000; then the
/// truncated `v / 65536`.
fn block_fixed(world: &World, p: [i64; 3]) -> [u8; 4] {
    let c = p.map(|v| {
        let hi = (v >> 32) as i32;
        let lo = v as u32 as i32;
        let v = if hi < 0 || (hi == 0 && lo < 0) { v.wrapping_sub(0x10000) } else { v };
        (v / 65536) as i32
    });
    world.block(c[0], c[1], c[2])
}

/// `Zone::getStaticCell(cx, cy, null)` 0x0042f640: the statics of the 8×8-block cell
/// `(cx, cy)` (`zone+0xac` list of `(cy & 31) * 32 + (cx & 31)`), `None` outside
/// `0..0x200000` or without the zone. The cell lists are filled when statics are added; they
/// are rebuilt here from the statics' positions in zone order (the same rule
/// `cw_sim::util::cell_statics` uses).
pub fn static_cell(world: &World, cx: i32, cy: i32) -> Option<Vec<&Static>> {
    if cx < 0 || cy < 0 || cx >= 0x20_0000 || cy >= 0x20_0000 {
        return None;
    }
    let zone = world.zone(cx >> 5, cy >> 5)?;
    Some(zone.statics.iter().filter(|s| (s.x >> 16) as i32 / 8 == cx && (s.y >> 16) as i32 / 8 == cy).collect())
}

/// The station test of `BlueprintPreviewWidget::update` 0x0042f910 (0x00431b1b.. per case)
/// for the local player `player` (`GC+0x8006d0 + 0x10`):
///
/// * [`Station::Water`] (0x00431b1b): the physics flag 2 (`entity+0x4c`, `creature+0x5c`),
///   or the block at `(x, y, z - (i64)(height * 0.5 * 65536) - (i64)6553.6)` (`height` =
///   `entity+0x78`, both `__ftol2`) is water (type 3).
/// * [`Station::Static`] (0x00432007.. for kinds 0x49, 0x4a, 0x47, 0x48, 0x4b; 0x00433cc0..
///   for 0x4c and 0x41): the cells `(pos / 65536) / 8 ± 1` (`__alldiv` then the truncating
///   `/ 8`, x outer) are searched in list order for a static of the kind whose squared
///   distance `(float)(static - player) / 65536` per axis (x87 `fild` rounded to float) is
///   below 16; the first form sums `dy² + dx² + dz²`, the second (`lengthSq` 0x00424860)
///   `dx² + dy² + dz²`, which is the same float.
pub fn station_ok(world: &World, player: &EntityData, st: Station) -> bool {
    let pos = [i64_at(player, 0), i64_at(player, 8), i64_at(player, 0x10)];
    match st {
        Station::Water => {
            if player.0[0x4c] & 2 != 0 {
                return true;
            }
            let h = f32_at(player, 0x78) * 0.5f32 * 65536.0f32;
            let z = pos[2].wrapping_sub(h as i64).wrapping_sub(6553.6f64 as i64);
            block_fixed(world, [pos[0], pos[1], z])[3] & 0x1f == 3
        }
        Station::Static { kind, .. } => {
            let cx = ((pos[0] / 0x10000) as i32) / 8;
            let cy = ((pos[1] / 0x10000) as i32) / 8;
            let k = 1.525_878_9e-5f32; // 0x006fcd94
            for x in cx - 1..=cx + 1 {
                for y in cy - 1..=cy + 1 {
                    let Some(list) = static_cell(world, x, y) else { continue };
                    for s in list {
                        if s.kind != kind {
                            continue;
                        }
                        let dx = (s.x.wrapping_sub(pos[0]) as f32) * k;
                        let dy = (s.y.wrapping_sub(pos[1]) as f32) * k;
                        let dz = (s.z.wrapping_sub(pos[2]) as f32) * k;
                        let sum = dy * dy + dx * dx + dz * dz;
                        if 16.0f32 > sum {
                            return true;
                        }
                    }
                }
            }
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> TextDb {
        let xml = r#"<root>
            <name key="Sword" tags="n"><singular>Sword</singular><plural>Swords</plural></name>
            <name key="Boots" tags="p m"><singular>Boots</singular></name>
            <name key="MaterialIron"><singular>Iron</singular><item>Iron @</item></name>
            <name key="item:shabby"><m>shabby @</m></name>
            <name key="item:exceptional2"><m>#'s exceptional @</m></name>
            <name key="item:polished2"><n>polished @ of #</n></name>
            <name key="LifePotion"><singular>Life Potion</singular></name>
            <name key="Key"><singular>Key</singular></name>
            <name key="GoblinMale" tags="m"><singular>Goblin</singular><plural>Goblins</plural></name>
            <name key="Trade Quarter"><singular>@ Trade Quarter</singular></name>
            <name key="Forest" tags="m in"><singular>@ Forest</singular></name>
            <speech key="objective:monster">Kill [@creature_singular $creature] in the [@scenery_singular $zone] !</speech>
            </root>"#;
        TextDb::from_xml(xml.as_bytes()).unwrap()
    }

    fn item(t: u8, sub: u8, material: u8, rarity: u8, modifier: u32) -> Vec<u8> {
        let mut v = vec![0u8; cw_net::entity::ITEM_SIZE];
        v[0] = t;
        v[1] = sub;
        v[4..8].copy_from_slice(&modifier.to_le_bytes());
        v[0xc] = rarity;
        v[0xd] = material;
        v[0x10] = 1;
        v
    }

    #[test]
    fn keys() {
        assert_eq!(creature_type_key(4), "GoblinMale");
        assert_eq!(creature_type_key(0x3e), "Mosquito");
        assert_eq!(creature_type_key(0x7777), "");
        assert_eq!(item_key(3, 1), "Axe");
        assert_eq!(item_key(0x14, 0x43), "ChocolateCookie");
        assert_eq!(item_key(0x14, 0x9b), "Bait");
        assert_eq!(item_key(0x14, 0x9c), "");
        assert_eq!(material_key(130), "MaterialIce");
        assert_eq!(material_key(3), "Material");
        assert_eq!(landscape_key(0xf, 9), "Island");
        assert_eq!(landscape_key(0xf, 10), "");
        assert_eq!(landscape_key(0xe, 3), "Palace");
        assert_eq!(site_key(1, 2), "Crafting Quarter");
        assert_eq!(adjective_key(4, 19), Some("item:fabulous"));
        assert_eq!(adjective_key(5, 1), None);
    }

    #[test]
    fn item_names() {
        let d = db();
        assert_eq!(item_name(Some(&d), &item(3, 0, 1, 0, 0)), "Iron Sword");
        // Rarity 0, modifier 10: "item:shabby" has only "m"; Sword's tag is "n" → first form.
        assert_eq!(item_name(Some(&d), &item(3, 0, 1, 0, 10)), "shabby Iron Sword");
        // Rarity 3, modifier 1: polished2 has "n" = Sword's tag; '#' is a generated name.
        let n = item_name(Some(&d), &item(3, 0, 1, 3, 1));
        assert_eq!(n, format!("polished Iron Sword of {}", generate_name(1, 7)));
        // No '@' in the material's item form (unknown material): the singular.
        assert_eq!(item_name(Some(&d), &item(3, 0, 3, 0, 0)), "Sword");
        assert_eq!(item_name(Some(&d), &item(1, 1, 0, 0, 0)), "Life Potion");
        assert_eq!(item_name(Some(&d), &item(0x13, 4, 0, 0, 0)), "Pet: GoblinMale");
        let mut f = item(2, 0, 1, 0, 0);
        f[8] = 3;
        assert_eq!(item_name(Some(&d), &f), "Formula: Iron Sword");
        let mut k = item(0x15, 3, 0, 0, 0);
        k[4..8].copy_from_slice(&5u32.to_le_bytes());
        k[8..12].copy_from_slice(&77u32.to_le_bytes());
        assert_eq!(item_name(Some(&d), &k), format!("{}'s Key", generate_name(77, 5)));
        assert_eq!(item_name(None, &item(3, 0, 1, 0, 0)), "");
    }

    #[test]
    fn map_names() {
        let d = db();
        let seeds = Seeds::from_seed(1234);
        let site = cw_world::region::ZoneRecord { kind: 1, sub: 1, seed: 99, level: 1, byte0c: 0 };
        assert_eq!(site_name(Some(&d), Some(&seeds), &site, 1000, 1000), format!("{} Trade Quarter", generate_name(99, -1)));
        let site = cw_world::region::ZoneRecord { seed: 0, ..site };
        let r = region_name(&seeds, 1000, 1000);
        assert!(!r.is_empty());
        assert_eq!(site_name(Some(&d), Some(&seeds), &site, 1000, 1000), format!("{r} Trade Quarter"));
        assert_eq!(region_name(&seeds, -1, 0), "");
        assert_eq!(cell_name(Some(&d), 3, 2, 5), format!("{} Forest", generate_name(5, -1)));
    }

    #[test]
    fn objective() {
        let d = db();
        let c = ObjectiveCell { kind: 3, variant: 0, id: 8, f30: 1, mission: 1, creature: 4 };
        let t = objective_text(Some(&d), &c);
        assert_eq!(t, format!("Kill Goblin in the {} Forest!", generate_name(8, -1)));
        assert_eq!(objective_text(Some(&d), &ObjectiveCell { mission: 6, ..c }), "");
    }

    /// The shipped dictionary, when the game folder is present (`CW_GAME_DIR` or `game/`).
    #[test]
    fn shipped_dictionary() {
        let dir = std::env::var_os("CW_GAME_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../game"));
        let Ok(adb) = cw_formats::AssetDb::open(dir.join("data4.db")) else { return };
        let Ok(d) = TextDb::load(&adb) else { return };
        assert_eq!(creature_singular(Some(&d), 4), "Goblin");
        assert_eq!(item_name(Some(&d), &item(3, 0, 1, 0, 0)), "Iron Sword");
        assert!(!item_name(Some(&d), &item(3, 0, 1, 4, 3)).is_empty());
        for (t, s, m, r, md) in [(3, 0, 1, 4, 3), (3, 0, 1, 3, 1), (6, 0, 25, 2, 7), (5, 0, 26, 1, 5), (0x15, 3, 0, 0, 0), (0x13, 0x1c, 0, 0, 0)] {
            eprintln!("item {t:#x}/{s} m{m} r{r} {md}: {}", item_name(Some(&d), &item(t, s, m, r, md)));
        }
        for (_, k) in ITEM_KEYS {
            let _ = form(&d, k, "singular");
        }
    }
}
