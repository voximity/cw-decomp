//! The interaction prompts of the crosshair text (`GameController::update`
//! `Cube.exe 0x00496922..0x00496f37`): "[R] Talk", "[R] Ride", "[R] Open", ... written into the
//! `crosshair` node (`GC+0x8008ac`, `Node::setText` 0x00636ad0) after the "[R] Revive" of a
//! dead player (0x004968d0), so a prompt replaces it.
//!
//! Tier B. The key handling interleaved with the prompts (R on the aimed creature: talk,
//! mount) is [`crate::interact::talk_or_mount`]; this module only chooses the text.
//!
//! # Map
//!
//! | Range | Piece |
//! |---|---|
//! | 0x00496922..0x00496938 | the aimed creature `GC+0x800a70` (`World::findEntity` 0x0042f000) |
//! | 0x0049693e..0x00496954 | hostility 3 with the cursor captured (`[esp+0x17]`, `isCursorFree` of the frame) → "[R] Talk" (0x00701be8); the static is not looked at |
//! | 0x00496a59..0x00496aab | hostility 5, a ridable type (0x00444760), the player's riding skill `entity+0x112c` set, the pet's parent `entity+0x188` the player's id, the player not riding (mode `entity+0x58` ≠ 0x6a) → "[R] Ride" (0x00701bfc); the static is not looked at |
//! | 0x00496b1e..0x00496b4d | otherwise the aimed static `GC+0x800a84` (0x005a0910); `kind - 1` through the byte table 0x0049d4ac / jump table 0x0049d47c |
//! | 0x00496b54 | kinds 1, 2, 3, 0xa (door, big door, window, chest): "[R] Open" (0x00701c10) when `Static+0x30` is set, else "[R] Close" (0x00701c24) |
//! | 0x00496bca.. | kinds 0x10, 0x12 (stool, bench) "[R] Sit Down"; 0x13, 0x44, 0x45 (bed, beach towel, sleeping mat) "[R] Sleep"; 0x2d (rune stone) "[R] Teleport"; 0x47..0x4d "[R] Use Furnace/Anvil/Spinning Wheel/Loom/Saw/Workbench/Customization Bench": none of them while the static's user `Static+0x40` (int64) is the player's id |
//! | 0x00496ef7 | every other kind (0, the campfire 0x41, ...): "[R] Examine" (0x00701d80) |

use cw_net::EntityData;
use cw_world::zone::Static;

fn i32_at(e: &EntityData, o: usize) -> i32 {
    i32::from_le_bytes(e.0[o..o + 4].try_into().unwrap())
}

fn i64_at(e: &EntityData, o: usize) -> i64 {
    i64::from_le_bytes(e.0[o..o + 8].try_into().unwrap())
}

/// The static's user (`Static+0x40`, the id of the creature sitting, sleeping or working on
/// it).
fn static_user(s: &Static) -> i64 {
    (u64::from(s.f40) | (u64::from(s.f44) << 32)) as i64
}

/// The prompt of an aimed static (0x00496b1e..0x00496f37), `None` when the player is its
/// user.
pub fn static_prompt(s: &Static, player_id: i64) -> Option<&'static str> {
    let used = || static_user(s) == player_id;
    let t = match s.kind {
        1 | 2 | 3 | 0xa => return Some(if s.b30 != 0 { "[R] Open" } else { "[R] Close" }),
        0x10 | 0x12 => "[R] Sit Down",
        0x13 | 0x44 | 0x45 => "[R] Sleep",
        0x2d => "[R] Teleport",
        0x47 => "[R] Use Furnace",
        0x48 => "[R] Use Anvil",
        0x49 => "[R] Use Spinning Wheel",
        0x4a => "[R] Use Loom",
        0x4b => "[R] Use Saw",
        0x4c => "[R] Use Workbench",
        0x4d => "[R] Use Customization Bench",
        _ => return Some("[R] Examine"),
    };
    if used() { None } else { Some(t) }
}

/// The interaction prompt of a frame (0x00496922..0x00496f37). `player` is the local
/// player's entity (`GC+0x8006d0 + 0x10`) with id `player_id`; `aimed_creature` the entity
/// of `GC+0x800a70` when it exists; `cursor_free` is `isCursorFree` (slot 1) as `update`
/// stored it in `[esp+0x17]`; `aimed_static` the static of `GC+0x800a84`. `None` leaves the
/// crosshair text as it was ("" or "[R] Revive").
pub fn interaction_prompt(
    player: &EntityData,
    player_id: i64,
    aimed_creature: Option<&EntityData>,
    cursor_free: bool,
    aimed_static: Option<&Static>,
) -> Option<&'static str> {
    if let Some(c) = aimed_creature {
        let hostile = c.0[0x50];
        // 0x00496941: a villager, cursor captured.
        if hostile == 3 && !cursor_free {
            return Some("[R] Talk");
        }
        // 0x00496a59: the player's own ridable pet.
        if hostile == 5
            && crate::interact::ridable_type(i32_at(c, 0x54))
            && i32_at(player, 0x112c) != 0
            && i64_at(c, 0x188) == player_id
            && player.0[0x58] != 0x6a
        {
            return Some("[R] Ride");
        }
    }
    aimed_static.and_then(|s| static_prompt(s, player_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(kind: u32) -> Static {
        Static { kind, x: 0, y: 0, z: 0, rotation: 0, scale: [1.0; 3], b30: 1, f34: 0, f38: 0, f40: 0, f44: 0, f54: 0, tail: [0; 6], inventory: Default::default() }
    }

    #[test]
    fn statics() {
        assert_eq!(static_prompt(&st(1), 7), Some("[R] Open"));
        assert_eq!(static_prompt(&Static { b30: 0, ..st(0xa) }, 7), Some("[R] Close"));
        assert_eq!(static_prompt(&st(0x13), 7), Some("[R] Sleep"));
        assert_eq!(static_prompt(&Static { f40: 7, ..st(0x13) }, 7), None);
        assert_eq!(static_prompt(&st(0x41), 7), Some("[R] Examine"));
        assert_eq!(static_prompt(&st(0), 7), Some("[R] Examine"));
        assert_eq!(static_prompt(&st(0x4e), 7), Some("[R] Examine"));
        assert_eq!(static_prompt(&st(0x4b), 7), Some("[R] Use Saw"));
    }

    #[test]
    fn creatures() {
        let p = EntityData::new_creature();
        let mut v = EntityData::new_creature();
        v.0[0x50] = 3;
        assert_eq!(interaction_prompt(&p, 1, Some(&v), false, None), Some("[R] Talk"));
        // Cursor free: the static decides.
        assert_eq!(interaction_prompt(&p, 1, Some(&v), true, Some(&st(0x12))), Some("[R] Sit Down"));
        let mut pet = EntityData::new_creature();
        pet.0[0x50] = 5;
        pet.0[0x54..0x58].copy_from_slice(&0x62i32.to_le_bytes());
        pet.0[0x188..0x190].copy_from_slice(&1i64.to_le_bytes());
        let mut rider = EntityData::new_creature();
        assert_eq!(interaction_prompt(&rider, 1, Some(&pet), false, None), None);
        rider.0[0x112c..0x1130].copy_from_slice(&1i32.to_le_bytes());
        assert_eq!(interaction_prompt(&rider, 1, Some(&pet), false, None), Some("[R] Ride"));
        rider.0[0x58] = 0x6a;
        assert_eq!(interaction_prompt(&rider, 1, Some(&pet), false, None), None);
    }
}
