//! `cube::MapOverlayWidget` (vtable 0x00702fe4, ctor 0x004c95a0, 0x164 bytes; `+0x160` the
//! `GameController*`): the text labels over the world map. Slot 1 `update` 0x004c9680 runs
//! while its node is drawn.
//!
//! The widget lives on `+0x8008a4`, a node under the separate root `+0x8008a0` (both made by
//! the ctor at 0x0045c090 / 0x0045c0d7, not under the GUI root). `render` 0x004ac260 shows the
//! overlay root only for the full map: with the map open (`GC+0x8006e4`) it hides the GUI
//! root, shows `+0x8008a0`, draws the GUI and returns (0x004ae220..0x004ae279); otherwise it
//! shows the GUI root when no menu screen is visible and hides `+0x8008a0`
//! (0x004ae27e..0x004ae369) — see [`overlay_roots`]. The 3D map itself (tiles, mission icons,
//! creature heads, the player arrow at `WorldMap+0x84`) is `cw_render::map`.
//!
//! Tier B.
//!
//! # `update` 0x004c9680
//!
//! 1. `GC+0x800dd4/+0x800dd8` (the hovered teleport zone) = −1.
//! 2. The centre zone: `trunc(trunc((player.x + (i64)(centre.x · 65536)) / 65536) / 256)`
//!    (same for y), `centre` the map offset `GC+0x1000e4c..+0x1000e54` (floats, blocks).
//! 3. Map zoom `GC+0x1c4 > 2`: for every zone in the 64×64 square `[c − 32, c + 32)` whose
//!    `WorldMap::zoneTile` 0x00602440 exists, is visited (`+0x30 & 1`) and has a site
//!    (`+0x10` kind ≠ 0): project the zone centre at `World::baseHeight` 0x005c5e20 and label
//!    it with the site name (0x004e5a20) in size 10, coloured by [`site_color`].
//! 4. For every such visited zone whose cell (`(zx / 8, zy / 8)`, 0x006023b0) has a type
//!    (`+0x18`) other than 0 and 10 and was not labelled yet (a `std::set` of cells), if
//!    `Cell::falloff` 0x005fa4c0 at the zone centre is > 0: label the cell once at its own
//!    position (`+0`, `+8`) with its name (0x004e5590) in size 12, coloured by [`cell_color`].
//!
//! Points are made relative to the player (minus the map offset; the cell labels do not
//! subtract the offset's z), converted with `(float)i64 · 2⁻¹⁶`, put through the map's
//! stored view (`WorldMap+0x44` = `GC+0x800d88`, divided by w, kept when `z > 0`) and the
//! stored projection (`WorldMap+4` = `GC+0x800d48`, divided by w, kept inside −1..1), then to
//! pixels `ndc · (W/2, −H/2) + (W/2, H/2)` with the engine viewport `Engine+0x10c/+0x110`.
//! Each label is two `drawText` calls (0x00639b30, font `resource1.dat`): an outline pass
//! (white fill, black outline, width 3) and the coloured fill.

/// A Direct3D-style row-major matrix (`v * M`), as `cw_render` stores `WorldMap+4`/`+0x44`.
pub type Matrix = [[f32; 4]; 4];

/// A zone record of the map cache (`cube::ZoneTile`, 0x34 bytes) as the overlay reads it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ZoneSite {
    /// `+0x30 & 1`: visited.
    pub visited: bool,
    /// `+0x10`: the site kind (0 none, 1 plain, 4 teleporter, others levelled sites).
    pub kind: u8,
    /// `+0x11`: the site sub type (the quarter of a town).
    pub sub: u8,
    /// `+0x14`: the site seed (its generated name).
    pub seed: i32,
    /// `+0x18`: the site level.
    pub level: i32,
}

impl ZoneSite {
    /// The overlay's view of a map-cache zone record (`cw_render::map::ZoneEntry`, whose
    /// `site` the landscape builder copies from the world region, 0x00603a00).
    pub fn from_entry(e: &cw_render::map::ZoneEntry) -> Self {
        ZoneSite { visited: e.flags & 1 != 0, kind: e.site.kind, sub: e.site.sub, seed: e.site.seed, level: e.site.level }
    }

    fn record(&self) -> cw_world::region::ZoneRecord {
        cw_world::region::ZoneRecord { kind: self.kind, sub: self.sub, seed: self.seed, level: self.level, byte0c: 0 }
    }
}

/// A cell (`cube::Cell`) as the overlay reads it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CellSite {
    /// `+0`, `+8`: position, fixed-point.
    pub x: i64,
    pub y: i64,
    /// `+0x18`: type (0 none, 1 home town, 10 skipped).
    pub kind: i32,
    /// `+0x1c`: variant (with the type, the landscape key 0x005a5240).
    pub variant: i32,
    /// `+0x20`: id (the seed of the generated name in the cell's label).
    pub id: i32,
    /// `+0x24`: level.
    pub level: i32,
}

impl CellSite {
    /// From a world cell.
    pub fn from_cell(c: &cw_world::region::Cell) -> Self {
        CellSite { x: c.x, y: c.y, kind: c.kind, variant: c.variant, id: c.id, level: c.level }
    }
}

/// What the default label texts read: the World's `cube::Speech` (`World+0x30`) and the
/// world's seeds (the region names of 0x005a6550).
#[derive(Clone, Copy, Default)]
pub struct NameSource<'a> {
    /// The dictionary (`GameUi::text`).
    pub text: Option<&'a super::textdb::TextDb>,
    /// `World+0x800164..`.
    pub seeds: Option<&'a cw_world::Seeds>,
}

/// The world and map queries of [`map_overlay`].
pub struct MapOverlayInputs<'a> {
    /// `GC+0x8006d0 + 0x10`: the player position (fixed).
    pub player_pos: [i64; 3],
    /// `entity+0x180` of the player.
    pub player_level: i32,
    /// `GC+0x1000e4c..+0x1000e54`: the map offset in blocks.
    pub centre: [f32; 3],
    /// `GC+0x1c4`: the map zoom (displayed; `+0x1c8` is its target).
    pub zoom: f32,
    /// `GC+0x800d88` (`WorldMap+0x44`): the stored map view /8.
    pub view: Matrix,
    /// `GC+0x800d48` (`WorldMap+4`): the stored map projection.
    pub projection: Matrix,
    /// `Engine+0x10c`, `Engine+0x110`.
    pub width: i32,
    pub height: i32,
    /// `Engine+0xd4`, `+0xd8`: the cursor.
    pub cursor: [f32; 2],
    /// `GC+0x800de4`: the map was opened from a teleporter (teleport targets are pickable).
    pub teleport_mode: bool,
    /// `GC+0x800ddc`: the picked teleporter zone (`0x00468840`, pair equality), drawn
    /// magenta (`crate::map_screen::TeleportPick::selected`).
    pub teleport_marked: &'a dyn Fn(i32, i32) -> bool,
    /// `WorldMap::zoneTile` 0x00602440.
    pub zone: &'a dyn Fn(i32, i32) -> Option<ZoneSite>,
    /// An override of the site name of a zone; `None` uses [`crate::names::site_name`]
    /// (0x004e5a20 over the zone's site record at `+0x10` and the zone centre in blocks,
    /// `zx * 256 + 128`).
    pub site_name: Option<&'a dyn Fn(i32, i32) -> String>,
    /// The cell by cell coordinates (0x006023b0).
    pub cell: &'a dyn Fn(i32, i32) -> Option<CellSite>,
    /// `Cell::falloff` 0x005fa4c0 of cell `(cx, cy)` at a fixed point.
    pub cell_falloff: &'a dyn Fn(i32, i32, i64, i64) -> f32,
    /// An override of the cell's name; `None` uses [`crate::names::cell_name`] (0x004e5590).
    pub cell_name: Option<&'a dyn Fn(i32, i32) -> String>,
    /// `World::baseHeight` 0x005c5e20 at a block (blocks).
    pub base_height: &'a dyn Fn(i32, i32) -> f32,
}

/// One label.
#[derive(Clone, Debug, PartialEq)]
pub struct MapLabel {
    /// Text.
    pub text: String,
    /// Pixel position (the `drawText` anchor; the call passes 2.0 as its alignment argument).
    pub position: [f32; 2],
    /// Size: 10 for sites, 12 for cells.
    pub size: f32,
    /// Fill colour of the second pass.
    pub color: [f32; 4],
    /// The first pass: white fill with a black outline of width 3.
    pub outline: f32,
}

/// What the overlay produced this frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MapOverlayFrame {
    /// Labels in draw order.
    pub labels: Vec<MapLabel>,
    /// `GC+0x800dd4/+0x800dd8`: the teleport zone under the cursor (`None` = −1, −1).
    pub hovered_zone: Option<(i32, i32)>,
}

/// Visibility of the GUI root `+0x800884` and the overlay root `+0x8008a0` while `render`
/// draws the widgets (0x004ae220 / 0x004ae334): `(gui_root, overlay_root)`.
/// `any_screen` is any of the start, character creation/select, server, world select/create
/// screens or the "Please wait..." node visible.
pub fn overlay_roots(map_open: bool, any_screen: bool) -> (bool, bool) {
    if map_open {
        (false, true)
    } else {
        (!any_screen, false)
    }
}

/// Site label colour (0x004c9f9d..0x004ca0f1): kind 1 white; kind 4 white, or in teleport mode
/// violet (0.6, 0.25, 1, 1), magenta (1, 0.25, 1, 1) when marked, cyan (0.2, 1, 1, 1) when
/// hovered; other kinds by level: `level > L + 2` red (1, 0.2, 0.2, 1), `level >= L − 2`
/// green (0.2, 1, 0.2, 1), else white.
pub fn site_color(kind: u8, level: i32, player_level: i32, teleport: Option<(bool, bool)>) -> [f32; 4] {
    let mut c = [1.0, 1.0, 1.0, 1.0];
    if kind == 4 {
        if let Some((marked, hovered)) = teleport {
            c = [0.6, 0.25, 1.0, 1.0];
            if marked {
                c = [1.0, 0.25, 1.0, 1.0];
            }
            if hovered {
                c = [0.2, 1.0, 1.0, 1.0];
            }
        }
    } else if kind != 1 && player_level - 2 <= level {
        c = if player_level + 2 < level { [1.0, 0.2, 0.2, 1.0] } else { [0.2, 1.0, 0.2, 1.0] };
    }
    c
}

/// Cell label colour (0x004ca9a6..): types 1 and 10 white; else by `levelCurve` 0x0043ca60:
/// `lc > lp + 0.1` red, `lc > lp − 0.1` cyan (0, 1, 1, 1), else white.
pub fn cell_color(kind: i32, level: i32, player_level: i32) -> [f32; 4] {
    use cw_world::generate::level_curve;
    if kind == 1 || kind == 10 {
        return [1.0; 4];
    }
    let lc = level_curve(level as f32);
    let lp = level_curve(player_level as f32);
    if lp - 0.1f32 < lc {
        if lp + 0.1f32 < lc {
            [1.0, 0.2, 0.2, 1.0]
        } else {
            [0.0, 1.0, 1.0, 1.0]
        }
    } else {
        [1.0; 4]
    }
}

/// `v * M` with the divide by w (the inlined transforms of 0x004c9680).
fn transform(m: &Matrix, v: [f32; 3]) -> [f32; 3] {
    let col = |j: usize| v[1] * m[1][j] + v[0] * m[0][j] + v[2] * m[2][j] + m[3][j];
    let inv = 1.0f32 / col(3);
    [col(0) * inv, col(1) * inv, col(2) * inv]
}

/// Map-space point to pixels, `None` when behind or off screen.
fn project(inp: &MapOverlayInputs, rel: [i64; 3]) -> Option<[f32; 2]> {
    let k = 1.525_878_9e-5f32;
    let v = [(rel[0] as f32) * k, (rel[1] as f32) * k, (rel[2] as f32) * k];
    let c = transform(&inp.view, v);
    if !(0.0f32 < c[2]) {
        return None;
    }
    let n = transform(&inp.projection, c);
    if !(-1.0f32 <= n[0] && n[0] <= 1.0f32 && -1.0f32 <= n[1] && n[1] <= 1.0f32) {
        return None;
    }
    let (w, h) = (inp.width as f32, inp.height as f32);
    Some([w * 0.5f32 * n[0] + w * 0.5f32, n[1] * (-h * 0.5f32) + h * 0.5f32])
}

/// `MapOverlayWidget::update` 0x004c9680; `names` feeds the default label texts.
pub fn map_overlay(inp: &MapOverlayInputs, names: &NameSource) -> MapOverlayFrame {
    let mut out = MapOverlayFrame::default();
    // `__ftol2` of `centre * 65536` (float product), truncating.
    let cf = [(inp.centre[0] * 65536.0f32) as i64, (inp.centre[1] * 65536.0f32) as i64, (inp.centre[2] * 65536.0f32) as i64];
    let p = inp.player_pos;
    let czx = (((p[0] + cf[0]) / 0x10000) as i32) / 256;
    let czy = (((p[1] + cf[1]) / 0x10000) as i32) / 256;

    // Part 1: site labels.
    if inp.zoom > 2.0f32 {
        for zx in czx - 32..czx + 32 {
            for zy in czy - 32..czy + 32 {
                let Some(z) = (inp.zone)(zx, zy) else { continue };
                if z.kind == 0 || !z.visited {
                    continue;
                }
                let bx = zx * 256 + 128;
                let by = zy * 256 + 128;
                let hz = ((inp.base_height)(bx, by) * 65536.0f32) as i64;
                let rel = [((bx as i64) << 16) - p[0] - cf[0], ((by as i64) << 16) - p[1] - cf[1], hz - p[2] - cf[2]];
                let Some(pos) = project(inp, rel) else { continue };
                let teleport = if z.kind == 4 && inp.teleport_mode {
                    let marked = (inp.teleport_marked)(zx, zy);
                    let [cx, cy] = inp.cursor;
                    let hovered = pos[0] - 100.0f32 <= cx && cx < pos[0] + 100.0f32 && pos[1] - 20.0f32 <= cy && cy < pos[1] + 10.0f32;
                    if hovered {
                        out.hovered_zone = Some((zx, zy));
                    }
                    Some((marked, hovered))
                } else {
                    None
                };
                let color = site_color(z.kind, z.level, inp.player_level, teleport);
                let text = match inp.site_name {
                    Some(f) => f(zx, zy),
                    None => crate::names::site_name(names.text, names.seeds, &z.record(), bx, by),
                };
                out.labels.push(MapLabel { text, position: pos, size: 10.0, color, outline: 3.0 });
            }
        }
    }

    // Part 2: cell labels, once per cell.
    let mut seen = std::collections::BTreeSet::new();
    for zx in czx - 32..czx + 32 {
        for zy in czy - 32..czy + 32 {
            let Some(z) = (inp.zone)(zx, zy) else { continue };
            if !z.visited {
                continue;
            }
            let (cx, cy) = (zx / 8, zy / 8);
            let Some(c) = (inp.cell)(cx, cy) else { continue };
            if c.kind == 0 || c.kind == 10 || seen.contains(&(cx, cy)) {
                continue;
            }
            let bx = zx * 256 + 128;
            let by = zy * 256 + 128;
            if !((inp.cell_falloff)(cx, cy, (bx as i64) << 16, (by as i64) << 16) > 0.0f32) {
                continue;
            }
            seen.insert((cx, cy));
            let hz = ((inp.base_height)((c.x / 0x10000) as i32, (c.y / 0x10000) as i32) * 65536.0f32) as i64;
            let rel = [c.x - p[0] - cf[0], c.y - p[1] - cf[1], hz - p[2]];
            let Some(pos) = project(inp, rel) else { continue };
            let color = cell_color(c.kind, c.level, inp.player_level);
            let text = match inp.cell_name {
                Some(f) => f(cx, cy),
                None => crate::names::cell_name(names.text, c.kind, c.variant, c.id),
            };
            out.labels.push(MapLabel { text, position: pos, size: 12.0, color, outline: 3.0 });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: Matrix = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];

    #[test]
    fn colors() {
        assert_eq!(site_color(2, 10, 5, None), [1.0, 0.2, 0.2, 1.0]);
        assert_eq!(site_color(2, 6, 5, None), [0.2, 1.0, 0.2, 1.0]);
        assert_eq!(site_color(2, 1, 5, None), [1.0; 4]);
        assert_eq!(site_color(1, 50, 5, None), [1.0; 4]);
        assert_eq!(site_color(4, 50, 5, None), [1.0; 4]);
        assert_eq!(site_color(4, 0, 5, Some((true, false))), [1.0, 0.25, 1.0, 1.0]);
        assert_eq!(site_color(4, 0, 5, Some((true, true))), [0.2, 1.0, 1.0, 1.0]);
        assert_eq!(cell_color(3, 20, 5), [1.0, 0.2, 0.2, 1.0]);
        assert_eq!(cell_color(3, 5, 5), [0.0, 1.0, 1.0, 1.0]);
        assert_eq!(cell_color(1, 50, 5), [1.0; 4]);
        assert_eq!(overlay_roots(true, false), (false, true));
        assert_eq!(overlay_roots(false, true), (false, false));
    }

    #[test]
    fn labels_project() {
        // View: move the map point 1 unit forward so z > 0; identity projection.
        let mut view = ID;
        view[3][2] = 1.0;
        let zone = |zx: i32, zy: i32| (zx == 0 && zy == 0).then(|| ZoneSite { visited: true, kind: 2, level: 5, ..Default::default() });
        let site = |_x: i32, _y: i32| "Cave".to_string();
        let cell = |cx: i32, cy: i32| (cx == 0 && cy == 0).then(|| CellSite { x: 128 << 16, y: 128 << 16, kind: 3, level: 5, ..Default::default() });
        let fall = |_cx: i32, _cy: i32, _x: i64, _y: i64| 1.0f32;
        let cname = |_cx: i32, _cy: i32| "Plains".to_string();
        let bh = |_x: i32, _y: i32| 0.0f32;
        let no = |_x: i32, _y: i32| false;
        let inp = MapOverlayInputs {
            player_pos: [128 << 16, 128 << 16, 0],
            player_level: 5,
            centre: [0.0; 3],
            zoom: 3.0,
            view,
            projection: ID,
            width: 800,
            height: 600,
            cursor: [400.0, 300.0],
            teleport_mode: false,
            teleport_marked: &no,
            zone: &zone,
            site_name: Some(&site),
            cell: &cell,
            cell_falloff: &fall,
            cell_name: Some(&cname),
            base_height: &bh,
        };
        let f = map_overlay(&inp, &NameSource::default());
        assert_eq!(f.labels.len(), 2);
        assert_eq!(f.labels[0].text, "Cave");
        assert_eq!(f.labels[0].position, [400.0, 300.0]);
        assert_eq!(f.labels[0].size, 10.0);
        assert_eq!(f.labels[1].text, "Plains");
        assert_eq!(f.labels[1].size, 12.0);
        assert_eq!(f.hovered_zone, None);
    }
}
