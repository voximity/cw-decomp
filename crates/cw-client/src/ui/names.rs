//! The creature/character name generator, `Cube.exe 0x005a0ed0` (a `cube::World` method in the
//! shared game code; `ecx` is the world, it is not read).
//!
//! `generateName(out, seed, race)` is deterministic: it draws no `rand()`. The name is one
//! prefix syllable plus one suffix syllable, both picked from per-race tables by two unsigned
//! 32-bit mixes of the seed:
//!
//! ```text
//! suffix = S[(seed * 7 + seed / 10) % n]      // 0x005a2f.. mul 0xcccccccd, lea, div
//! prefix = P[(seed / 7 + seed * 13) % n]      // mul 0x24924925, imul 0xd, div
//! name   = prefix + suffix                     // std::wstring operator+ 0x0058d7a0
//! ```
//!
//! All arithmetic is `uint32` (the seed is the `uint` argument; `mul`/`div`, not `imul`/`idiv`).
//! `race` is compared unsigned against 0x54 (`cmp eax, 0x54; ja default`), then dispatched
//! through the byte table 0x005a3574 and the jump table 0x005a3530. Races 2, 0x12, 0x2b and
//! 0x53 share the human-male tables, 3, 0x2d and 0x54 the human-female tables; every race
//! without an entry (6, 0x11, 0x13.., and anything above 0x54) uses the 20-entry default
//! tables.
//!
//! The syllables are the static `std::wstring` arrays the function builds on first use
//! (guard bits in 0x0076b9e8 / 0x0076d8e0). There are 360 entries in 36 arrays (280 distinct
//! strings, 73 of them long enough to show up in a plain string scan: the "73 syllables" of
//! `client-map.md`). Tables read from the binary with the destination/source pairs of the
//! 360 `std::wstring(const wchar_t*)` calls (0x0040eb60).
//!
//! Callers: `GameController::update` 0x0049a95e / 0x0049a9ce (creature name labels:
//! `generateName(entity id low dword, entity type)`), the quest tag text 0x00477fa0
//! (`@name` of an objective), the item namer 0x00598a50 (pets), 0x005953a0 and the speech
//! helpers 0x004e5320 / 0x004e5590 / 0x004e5a20.

/// One race's syllable tables: `(prefixes, suffixes)` with their addresses.
struct Tables {
    prefix: &'static [&'static str],
    suffix: &'static [&'static str],
}

// 0x0076b808 / 0x0076b9f0 (20 entries each): the default tables.
const DEFAULT: Tables = Tables {
    prefix: &[
        "Kro", "Aru", "As", "Ge", "Kur", "Lugo", "Iko", "Liku", "Tero", "Var", "An", "The", "Ga", "Lan", "Dura",
        "Dama", "Se", "Thal", "Nar", "San",
    ],
    suffix: &[
        "no", "mi", "la", "sel", "ron", "ion", "kor", "lon", "rok", "tar", "den", "gar", "dar", "dara", "lan", "ka",
        "gor", "rior", "ria", "mor",
    ],
};

// Race 0 (elf male): 0x0076bf90 / 0x0076c080.
const ELF_M: Tables = Tables {
    prefix: &["El", "Li", "Dri", "Elan", "Le", "Ly", "A", "Tho", "Da", "Les"],
    suffix: &["ric", "kun", "zic", "dor", "las", "sander", "reon", "reas", "lundra", "andor"],
};
// Race 1 (elf female): 0x0076c170 / 0x0076c260.
const ELF_F: Tables = Tables {
    prefix: &["Ar", "A", "Si", "Mer", "Lu", "Ari", "Su", "Sira", "Hia", "Li"],
    suffix: &["weya", "luna", "laya", "leya", "ma", "lia", "matra", "vy", "zyna", "ly"],
};
// Races 2, 0x12, 0x2b, 0x53 (human male): 0x0076bbd0 / 0x0076bcc0.
const HUMAN_M: Tables = Tables {
    prefix: &["De", "Ro", "As", "Ge", "An", "Pat", "Wolf", "Ale", "Mi", "Ben"],
    suffix: &["rek", "man", "gram", "rald", "rick", "ram", "sander", "kal", "ton", "ny"],
};
// Races 3, 0x2d, 0x54 (human female): 0x0076bdb0 / 0x0076bea0.
const HUMAN_F: Tables = Tables {
    prefix: &["Sa", "Jen", "Kla", "Mi", "La", "Auri", "Mela", "Alu", "Ve", "Est"],
    suffix: &["rah", "ny", "ria", "elle", "na", "ga", "ma", "riana", "rona", "una"],
};
// Race 4 (goblin male): 0x0076c350 / 0x0076c440.
const GOBLIN_M: Tables = Tables {
    prefix: &["Gra", "Xe", "Ur", "Bel", "Chu", "Ki", "Go", "Zim", "Kubo", "Raz"],
    suffix: &["zic", "nax", "tik", "tuk", "ruk", "bo", "rix", "mi", "tok", "nor"],
};
// Race 5 (goblin female): 0x0076c530 / 0x0076c620.
const GOBLIN_F: Tables = Tables {
    prefix: &["Kur", "Xe", "Ki", "Am", "Zifa", "Zu", "Ma", "Chi", "Zi", "Mia"],
    suffix: &["ra", "lia", "bi", "ba", "ly", "ki", "bara", "mi", "zy", "xa"],
};
// Race 7 (lizard male): 0x0076c710 / 0x0076c800.
const LIZARD_M: Tables = Tables {
    prefix: &["Cho", "Zer", "Kaz", "Kraz", "Zel", "Drak", "Lo", "Raz", "Spi", "Zen"],
    suffix: &["rux", "rek", "zic", "tac", "mec", "kor", "rax", "zor", "ro", "go"],
};
// Race 8 (lizard female): 0x0076c8f0 / 0x0076c9e0.
const LIZARD_F: Tables = Tables {
    prefix: &["Zi", "Zer", "Az", "Ki", "Iza", "Drak", "Li", "May", "Spi", "Zan"],
    suffix: &["rya", "ri", "zia", "ki", "mah", "ira", "maya", "ana", "ra", "ya"],
};
// Race 9 (dwarf male): 0x0076cad0 / 0x0076cbc0.
const DWARF_M: Tables = Tables {
    prefix: &["Tor", "Gem", "Bar", "Me", "As", "Tem", "Arak", "Ur", "Grim", "Xor"],
    suffix: &["lox", "bart", "kor", "thos", "kun", "bur", "thor", "lok", "li", "bor"],
};
// Race 10 (dwarf female): 0x0076ccb0 / 0x0076cda0.
const DWARF_F: Tables = Tables {
    prefix: &["Mum", "Grun", "Brun", "Hei", "Mo", "Hil", "Ur", "Lum", "Grim", "Xor"],
    suffix: &["pie", "hild", "di", "muna", "ki", "trud", "sa", "axa", "ira", "ika"],
};
// Race 11 (orc male): 0x0076ce90 / 0x0076cf80.
const ORC_M: Tables = Tables {
    prefix: &["Uz", "Ur", "Chu", "Ku", "Mor", "Ura", "Ak", "Ur", "Or", "Gor"],
    suffix: &["ku", "ruk", "muk", "thak", "dok", "tor", "rorok", "chak", "kaz", "rack"],
};
// Race 12 (orc female): 0x0076d070 / 0x0076d160.
const ORC_F: Tables = Tables {
    prefix: &["Uz", "Ur", "Chu", "Ku", "Mor", "Ura", "Ak", "Ur", "Or", "Gor"],
    suffix: &["ka", "rua", "mua", "thara", "daka", "tah", "rorah", "chaka", "kaya", "rana"],
};
// Race 13 (frogman male): 0x0076d610 / 0x0076d700.
const FROG_M: Tables = Tables {
    prefix: &["Qua", "Rib", "Quib", "Zib", "Il", "Wok", "Wib", "Moko", "Sli", "Bul"],
    suffix: &["rik", "bit", "ble", "bik", "wak", "wok", "wib", "luk", "mey", "bak"],
};
// Race 14 (frogman female): 0x0076d7f0 / 0x0076d8e8.
const FROG_F: Tables = Tables {
    prefix: &["Qua", "Rib", "Quib", "Zib", "Il", "Wok", "Wib", "Moko", "Sli", "Bul"],
    suffix: &["ria", "bia", "bla", "bia", "waka", "woka", "wibba", "lua", "maya", "ba"],
};
// Race 15 (undead male): 0x0076d250 / 0x0076d340.
const UNDEAD_M: Tables = Tables {
    prefix: &["Chu", "Dro", "Al", "Dem", "Mor", "Mur", "Cro", "Zul", "Dra", "The"],
    suffix: &["lu", "kor", "card", "morius", "enius", "tus", "demar", "lus", "ruul", "zad"],
};
// Race 16 (undead female): 0x0076d430 / 0x0076d520.
const UNDEAD_F: Tables = Tables {
    prefix: &["Xu", "Dra", "Al", "Dem", "Mor", "Myr", "Cra", "Zul", "Dri", "Thu"],
    suffix: &["la", "kira", "cara", "moria", "ena", "tana", "diria", "laza", "rah", "zazah"],
};

/// The switch of 0x005a2f42 (`cmp eax, 0x54; ja default` + byte table 0x005a3574).
fn tables(race: i32) -> &'static Tables {
    match race as u32 {
        0 => &ELF_M,
        1 => &ELF_F,
        2 | 0x12 | 0x2b | 0x53 => &HUMAN_M,
        3 | 0x2d | 0x54 => &HUMAN_F,
        4 => &GOBLIN_M,
        5 => &GOBLIN_F,
        7 => &LIZARD_M,
        8 => &LIZARD_F,
        9 => &DWARF_M,
        10 => &DWARF_F,
        11 => &ORC_M,
        12 => &ORC_F,
        13 => &FROG_M,
        14 => &FROG_F,
        15 => &UNDEAD_M,
        16 => &UNDEAD_F,
        _ => &DEFAULT,
    }
}

/// `generateName` 0x005a0ed0: the name for `seed` (an entity id's low dword, an objective
/// seed) and `race` (the entity type, `entity+0x54`).
pub fn generate_name(seed: u32, race: i32) -> String {
    let t = tables(race);
    let n = t.prefix.len() as u32;
    // Suffix index: `(seed * 7 + seed / 10) % n`, all uint32.
    let s = seed.wrapping_mul(7).wrapping_add(seed / 10) % n;
    // Prefix index: `(seed / 7 + seed * 13) % n`.
    let p = (seed / 7).wrapping_add(seed.wrapping_mul(13)) % n;
    let mut out = String::with_capacity(12);
    out.push_str(t.prefix[p as usize]);
    out.push_str(t.suffix[s as usize]);
    out
}

// ---------------------------------------------------------------------------------------
// The HUD's landscape names (`Speech` methods, `ecx` = `GC+0x314`)
// ---------------------------------------------------------------------------------------
//
// `update` writes the 0x004e5320 result ([`area_name`]: the cell's name or "Lands of X")
// into the big size-20 `landscape` line and the `landname` banner's `name`, and the
// 0x004e5c10 result ([`zone_site_name`]: the zone's site, e.g. "Trade District") into the
// small size-12 `landscapedetail` line and the banner's `detail` (0x004942ca / 0x00494314,
// 0x004943e1 / 0x0049442b).

/// `0x004a6ad0(zx, zy)`: the 16-byte zone record of region `(zx / 64, zy / 64)` at
/// `Region+0x18 + ((zx % 64) * 64 + zy % 64) * 16`; `None` outside `0..0x10000` or when the
/// region is not loaded (the record is returned whatever its kind).
pub fn zone_record(world: &cw_world::World, zx: i32, zy: i32) -> Option<cw_world::region::ZoneRecord> {
    if !(0..0x10000).contains(&zx) || !(0..0x10000).contains(&zy) {
        return None;
    }
    let r = world.region(zx / 64, zy / 64)?;
    r.zones.get(((zx % 64) * 64 + zy % 64) as usize).copied()
}

/// `Speech::landscapeName(out, world, x, y)` `Cube.exe 0x004e5c10` over an already looked-up
/// zone record: `""` without one, else `Speech::siteName(out, world, record, x, y)`
/// 0x004e5a20 ([`crate::names::site_name`]) with the block position itself (not the zone
/// centre the map labels pass).
pub fn zone_site_name_of(
    db: Option<&super::textdb::TextDb>,
    seeds: Option<&cw_world::Seeds>,
    record: Option<&cw_world::region::ZoneRecord>,
    x: i32,
    y: i32,
) -> String {
    match record {
        // 0x004e5c66: `siteName(out, world, record, x, y)`.
        Some(r) => crate::names::site_name(db, seeds, r, x, y),
        // 0x004e5c4f: L"" (0x006fccac).
        None => String::new(),
    }
}

/// `Cube.exe 0x004e5c10`: the landscape name of block `(x, y)`: the zone record of
/// `(x / 256, y / 256)` (C division, 0x004e5c17..0x004e5c35) through [`zone_record`], then
/// [`zone_site_name_of`].
pub fn zone_site_name(db: Option<&super::textdb::TextDb>, world: &cw_world::World, x: i32, y: i32) -> String {
    let rec = zone_record(world, x / 256, y / 256);
    zone_site_name_of(db, Some(&world.seeds), rec.as_ref(), x, y)
}

/// `Speech::landscapeDetail(out, world, x, y)` `Cube.exe 0x004e5320` over the looked-up
/// parts, in the original's order:
///
/// * `cell` is `World::getCell` 0x00487da0 of `((x / 256) / 8, (y / 256) / 8)` as
///   `(kind +0x18, variant +0x1c, id +0x20)`. When it exists and its kind is neither 0 nor
///   10 (0x004e53a5..0x004e53af), `falloff()` (`Cell::falloff` 0x005fa4c0 at
///   `(x << 16, y << 16)`) is evaluated; `!(0.2 > falloff)` (`comiss` + `ja`, so NaN takes
///   the cell) gives `Speech::cellName` 0x004e5590 ([`crate::names::cell_name`]).
/// * Otherwise `point()` is `World::nearestClimatePoint` 0x00477e10 at `(x, y)` as
///   `(seed +0x14, elevation +0x18)`; none gives `""`. The text is the `landscape` entry
///   `normal` of `"Lands of"` (`Speech+0xc[L"Lands of"]`, 0x004689a0; a missing key reads
///   `""`), replaced by that of `"Ocean"` when the elevation is negative (0x004e5493
///   `jge`). Empty gives `""`; without an `@` it also gives `""` (0x004e550d); else the
///   first `@` becomes `generateName(seed, -1)` 0x005a0ed0.
///
/// No `rand()` draw.
pub fn area_name_of(
    db: Option<&super::textdb::TextDb>,
    cell: Option<(i32, i32, i32)>,
    falloff: impl FnOnce() -> f32,
    point: impl FnOnce() -> Option<(i32, i32)>,
) -> String {
    if let Some((kind, variant, id)) = cell
        && kind != 0
        && kind != 10
    {
        let f = falloff();
        // 0x004e53ed: `comiss 0.2, f; ja fallback`.
        if !(0.2f32 > f) {
            return crate::names::cell_name(db, kind, variant, id);
        }
    }
    // 0x004e5403: the nearest climate point.
    let Some((seed, elevation)) = point() else { return String::new() };
    let normal = |key: &str| db.and_then(|d| d.landscapes.get(key)).map(|e| e.normal.clone()).unwrap_or_default();
    // 0x004e5427: `Speech+0xc` is the `landscape` entries' `normal` map (ctor 0x004e2254).
    let mut s = normal("Lands of");
    if elevation < 0 {
        // 0x004e5498.
        s = normal("Ocean");
    }
    if s.is_empty() {
        return String::new();
    }
    // 0x004e54ef: `find(L'@')`, then `replace(pos, 1, generateName(point+0x14, -1))`.
    let Some(p) = s.find('@') else { return String::new() };
    s.replace_range(p..p + 1, &generate_name(seed as u32, -1));
    s
}

/// `Cube.exe 0x004e5320` on the world: [`area_name_of`] with the cell of
/// `((x / 256) / 8, (y / 256) / 8)` (0x004e534e..0x004e539a), its falloff at the fixed
/// position `(x << 16, y << 16)` (0x004e53b1..0x004e53dd, sign-extended `shld`/`shl`) and
/// the nearest climate point.
pub fn area_name(db: Option<&super::textdb::TextDb>, world: &cw_world::World, x: i32, y: i32) -> String {
    let cell = world.cell((x / 256) / 8, (y / 256) / 8);
    area_name_of(
        db,
        cell.map(|c| (c.kind, c.variant, c.id)),
        || cell.map_or(0.0, |c| c.falloff(i64::from(x) << 16, i64::from(y) << 16)),
        || world.nearest_climate_point(x, y).map(|p| (p.seed, p.elevation)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use cw_math::rand::MsvcRand;

    /// Golden: the first 20 (seed, race) pairs of the default MSVC stream (`srand(1)`),
    /// `seed = rand()`, `race = rand() % 17`, against a Python model built directly from the
    /// Cube.exe tables (scratch script `ngold.py`, tables read from the 360 constructor calls
    /// and the jump table 0x005a3530).
    #[test]
    fn golden_first_twenty() {
        let expected: [(i32, i32, &str); 20] = [
            (41, 5, "Zilia"),
            (6334, 14, "Wibbia"),
            (19169, 16, "Myrzazah"),
            (11478, 16, "Demmoria"),
            (26962, 1, "Siraweya"),
            (5705, 10, "Mumtrud"),
            (23281, 14, "Sliwoka"),
            (9961, 15, "Cromorius"),
            (2995, 8, "Azmah"),
            (4827, 13, "Quabit"),
            (32391, 1, "Armatra"),
            (3902, 0, "Elanlas"),
            (292, 6, "Thaldara"),
            (17421, 16, "Drazazah"),
            (19718, 5, "Kurmi"),
            (5447, 0, "Lesdor"),
            (14771, 12, "Kudaka"),
            (1869, 5, "Zifaxa"),
            (25667, 0, "Thosander"),
            (17035, 0, "Dalundra"),
        ];
        let mut r = MsvcRand::default();
        for (seed, race, name) in expected {
            let s = r.rand();
            let rc = r.rand() % 17;
            assert_eq!((s, rc), (seed, race));
            assert_eq!(generate_name(s as u32, rc), name);
        }
    }

    #[test]
    fn unsigned_arithmetic_and_aliases() {
        // Large seeds wrap as uint32 (0x005a2f..: `mul`, not `imul`).
        assert_eq!(generate_name(0xffff_ffff, 2), "Gegram");
        assert_eq!(generate_name(0xffff_ffff - 977, 2), "Asram");
        assert_eq!(generate_name(0xffff_ffff - 2 * 977, 2), "Asny");
        // 0x53 shares the human-male tables.
        assert_eq!(generate_name(0, 0x53), "Derek");
        assert_eq!(generate_name(1, 0x53), "Gekal");
        assert_eq!(generate_name(2, 0x53), "Wolfrick");
        // Above 0x54 (and negative, compared unsigned): the default tables.
        assert_eq!(generate_name(12345, 99), "Terotar");
        assert_eq!(generate_name(12345, -1), "Terotar");
    }

    fn landscape_db() -> crate::ui::textdb::TextDb {
        let xml = r#"<root>
<name key="Portal"><singular>Portal of @</singular></name>
<name key="Palace"><singular>Royal Palace</singular></name>
<name key="Ruins"><singular>Ruins of @</singular></name>
<landscape key="Lands of"><normal>Lands of @</normal></landscape>
<landscape key="Ocean"><normal>@ Ocean</normal></landscape>
</root>"#;
        crate::ui::textdb::TextDb::from_xml(xml.as_bytes()).unwrap()
    }

    /// 0x004e5c10: no record gives ""; a site record goes through `siteName` 0x004e5a20
    /// (`@` = `generateName(seed, -1)` when seed and kind are set).
    #[test]
    fn zone_site_name_from_site_record() {
        use cw_world::region::ZoneRecord;
        let db = landscape_db();
        assert_eq!(zone_site_name_of(Some(&db), None, None, 0, 0), "");
        let portal = ZoneRecord { kind: 4, sub: 0, seed: 12345, ..ZoneRecord::DEFAULT };
        assert_eq!(zone_site_name_of(Some(&db), None, Some(&portal), 100, 200), "Portal of Terotar");
        let palace = ZoneRecord { kind: 5, sub: 0, seed: 7, ..ZoneRecord::DEFAULT };
        assert_eq!(zone_site_name_of(Some(&db), None, Some(&palace), 0, 0), "Royal Palace");
        // Kind 0 has no site key: "".
        let none = ZoneRecord { kind: 0, sub: 0, seed: 7, ..ZoneRecord::DEFAULT };
        assert_eq!(zone_site_name_of(Some(&db), None, Some(&none), 0, 0), "");
        // No dictionary: "".
        assert_eq!(zone_site_name_of(None, None, Some(&portal), 0, 0), "");
    }

    /// 0x004e5320: the cell name when the cell's kind is set (not 10) and its falloff is not
    /// below 0.2, else "Lands of" / "Ocean" with the climate point's name.
    #[test]
    fn area_name_rules() {
        let db = landscape_db();
        let no_point = || None;
        // Ruins cell (5, 0) at full strength: `cellName`, `@` = generateName(id, -1).
        assert_eq!(area_name_of(Some(&db), Some((5, 0, 12345)), || 1.0, no_point), "Ruins of Terotar");
        // Exactly 0.2 still takes the cell (`0.2 > f` is false); NaN too.
        assert_eq!(area_name_of(Some(&db), Some((5, 0, 12345)), || 0.2, no_point), "Ruins of Terotar");
        assert_eq!(area_name_of(Some(&db), Some((5, 0, 12345)), || f32::NAN, no_point), "Ruins of Terotar");
        // Weak cell, kind 0 or kind 10: the climate point.
        let land = || Some((12345, 30));
        assert_eq!(area_name_of(Some(&db), Some((5, 0, 1)), || 0.19, land), "Lands of Terotar");
        assert_eq!(area_name_of(Some(&db), Some((10, 0, 1)), || panic!("falloff read"), land), "Lands of Terotar");
        assert_eq!(area_name_of(Some(&db), Some((0, 0, 1)), || panic!("falloff read"), land), "Lands of Terotar");
        assert_eq!(area_name_of(Some(&db), None, || panic!("falloff read"), land), "Lands of Terotar");
        // Negative elevation: "Ocean".
        assert_eq!(area_name_of(Some(&db), None, || 0.0, || Some((12345, -1))), "Terotar Ocean");
        // No climate point, no dictionary: "".
        assert_eq!(area_name_of(Some(&db), None, || 0.0, no_point), "");
        assert_eq!(area_name_of(None, None, || 0.0, land), "");
    }

    /// 0x004e550d: a "Lands of" text without an `@` gives "", not the text.
    #[test]
    fn area_name_without_at_is_empty() {
        let xml = r#"<root><landscape key="Lands of"><normal>Wilderness</normal></landscape></root>"#;
        let db = crate::ui::textdb::TextDb::from_xml(xml.as_bytes()).unwrap();
        assert_eq!(area_name_of(Some(&db), None, || 0.0, || Some((1, 1))), "");
    }
}
