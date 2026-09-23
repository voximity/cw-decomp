//! The per-column decoration pass of `cube::World::generateZone` (`Server.exe 0x00518630`,
//! instructions `0x0051fa20..0x005210a6`): props (flowers, grass tufts, lilies, water plants),
//! the kind 0x32/0x33 statics and the ground items scattered on the surface of every column.
//!
//! For each column the pass walks the block array bottom-up and decorates every index `i` whose
//! block above (`i + 1`) is not solid (`FUN_004061f0`: type is neither 0 nor 2), plus the top
//! block. `h = base + i + 1` is the height just above block `i`.

use cw_math::value_noise_2d as noise;

use crate::fixed::{add_double, from_block, sub_double};
use crate::appearance::Item;
use crate::world::World;
use crate::zone::{GroundItem, Prop, Static, Zone};

/// `FUN_004061f0`: the block is solid (type neither air 0 nor water 2).
#[inline]
fn solid(t: u8) -> bool {
    t != 0 && t != 2
}

#[inline]
fn prop_at(x: i32, y: i32, h: i32) -> Prop {
    Prop { x: add_double(from_block(x), 0.5), y: add_double(from_block(y), 0.5), z: from_block(h), ..Prop::NEW }
}

/// `cube::GroundItem::GroundItem` (`Server.exe 0x0041d8d0`) defaults for the kept fields, placed
/// at `(x, y, h)`.
#[inline]
fn item_at(x: i32, y: i32, h: i32) -> GroundItem {
    GroundItem {
        item: Item::NEW,
        x: add_double(from_block(x), 0.5),
        y: add_double(from_block(y), 0.5),
        z: from_block(h),
        rotation: 0.0,
        f134: f32::from_bits(0x3d92_4925),
        b138: 2,
        ..GroundItem::NEW
    }
}

impl World {
    /// The column decoration loop of `generateZone` (`Server.exe 0x0051fa20..0x005210a6`), run
    /// right after the 3x3 creature spawn grid.
    pub fn decorate_columns(&mut self, zone: &mut Zone) {
        let x0 = zone.x * 256;
        let y0 = zone.y * 256;
        // local_1420 = 470, local_13f8 = 150, local_1334 = 300 (0x51fa0a..0x51fa2a).
        for x in x0..x0 + 256 {
            for y in y0..y0 + 256 {
                let mut i = 0;
                while i < zone.column(x, y).blocks.len() as i32 {
                    let col = zone.column(x, y);
                    let count = col.blocks.len() as i32;
                    let skip = i < count - 1 && solid(col.block_at(i + 1)[3] & 0x1f);
                    if !skip {
                        let t = col.block_at(i)[3] & 0x1f;
                        let next = col.block_at(i + 1)[3] & 0x1f;
                        let h = i + col.height + 1;
                        self.decorate_block(zone, x, y, h, t, next);
                    }
                    i += 1;
                }
            }
        }
    }

    /// Body of the loop for one column index (`0x0051fac0..0x00520371`).
    fn decorate_block(&mut self, zone: &mut Zone, x: i32, y: i32, h: i32, t: u8, next: u8) {
        if next == 0 {
            match t {
                0xb => {
                    self.decorate_static(zone, x, y, h);
                    return;
                }
                3 => {
                    let b = zone.column(x, y).climate_b;
                    if b > 0.2 {
                        let n = noise(f64::from(x as f32 * 0.05 + 9843.0), f64::from(y as f32 * 0.05 + 8437.0)).abs();
                        if n > 0.5 && self.rng.rand() % 8 == 0 {
                            // 0x51fdab
                            let mut p = prop_at(x, y, h);
                            let r = self.rng.rand(); // 0x51fe70
                            p.rotation = (r as f32 * 360.0) / 32767.0;
                            p.scale = 0.09;
                            p.kind = 0x16;
                            p.flags = 4; // assigned, not ORed (0x51feb3)
                            zone.props.push(p);
                        }
                    }
                    return;
                }
                2 => {
                    let b = zone.column(x, y).climate_b;
                    if b > 0.2 && h > 0 {
                        let n = noise(f64::from(x as f32 * 0.05 + 24234.0), f64::from(y as f32 * 0.05 + 53565.0)).abs();
                        if n > 0.7 && self.rng.rand() % 10 == 0 {
                            // 0x51ff8b
                            let mut p = prop_at(x, y, h);
                            p.z = sub_double(from_block(h), 0.1);
                            let r = self.rng.rand(); // 0x520078
                            p.scale = 0.09;
                            p.rotation = (r as f32 * 360.0) / 32767.0;
                            p.kind = (self.rng.rand() % 2) as u32 + 0x1f; // 0x5200a7
                            zone.props.push(p);
                        }
                    }
                    return;
                }
                _ => {}
            }
        }

        // 0x005200d6
        if !matches!(t, 4 | 9 | 0xc | 0xa) {
            return;
        }
        let fx = x as f32;
        let fy = y as f32;
        let n = noise(f64::from(fx * 0.05 + 9843.0), f64::from(fy * 0.05 + 8437.0)).abs();
        if n <= 0.6 {
            return;
        }
        if self.rng.rand() % 8 != 0 {
            // 0x52017d
            return;
        }

        if h < 1 {
            if h > -5 {
                return;
            }
            // Underwater plants, kinds 5/6/7 (0x5201ac).
            let mut p = prop_at(x, y, h);
            let q = self.rng.rand() % 4; // 0x520254
            p.rotation = (q * 90) as f32;
            p.scale = 0.1;
            let n = noise(f64::from(x) * 0.01 + 9843.0, f64::from(y) * 0.01 + 8437.0);
            if n <= 0.0 {
                p.kind = 7;
                p.scale = 0.1;
                p.flags |= 4;
            } else {
                p.kind = (self.rng.rand() % 2) as u32 + 5; // 0x5202e9
                if p.kind == 5 {
                    p.scale = 0.075;
                    p.flags |= 4;
                }
            }
            let r = self.rng.rand(); // 0x520333
            p.scale *= r as f32 / 32767.0 + 1.0;
            zone.props.push(p);
            return;
        }

        if t == 0xc {
            // Kinds 9/10 (0x520469).
            let mut p = prop_at(x, y, h);
            let q = self.rng.rand() % 4; // 0x520511
            p.rotation = (q * 90) as f32;
            p.scale = 0.075;
            let n = noise(f64::from(x) * 0.01 + 9843.0, f64::from(y) * 0.01 + 8437.0);
            p.kind = u32::from(n <= 0.0) + 9;
            if p.kind == 9 {
                p.flags |= 4;
            } else if p.kind == 10 {
                let r = self.rng.rand(); // 0x5205c8
                p.scale = ((f64::from(r) * 0.02) / 32767.0 + 0.03) as f32;
            }
            zone.props.push(p);
            return;
        }

        if t != 0xa {
            let b = zone.column(x, y).climate_b;
            if b <= 0.75 {
                // Kinds 0..4 / 0xc (0x520b7b).
                let mut p = prop_at(x, y, h);
                let q = self.rng.rand() % 4; // 0x520c28
                p.rotation = (q * 90) as f32;
                p.scale = 0.075;
                let a = zone.column(x, y).climate_a;
                let dx = f64::from(x) * 0.01;
                let dy = f64::from(y) * 0.01;
                let n = noise(dx + 9843.0, dy + 8437.0);
                p.kind = if a <= 0.5 {
                    if n <= 0.0 {
                        let n = noise(dx + 34234.0, dy + 234234.0);
                        u32::from(n <= 0.0)
                    } else {
                        let n = noise(f64::from(fx * 0.01 + 34234.0), f64::from(fy * 0.01 + 234234.0));
                        u32::from(n <= 0.0) + 2
                    }
                } else if n <= 0.0 {
                    let n = noise(dx + 34234.0, dy + 234234.0);
                    u32::from(n <= 0.0)
                } else {
                    let n = noise(f64::from(fx * 0.01 + 34234.0), f64::from(fy * 0.01 + 234234.0));
                    u32::from(n > 0.0) * 8 + 4
                };
                if matches!(p.kind, 2 | 3 | 4 | 0xc) {
                    p.flags |= 4;
                    if p.kind == 0xc {
                        let r = self.rng.rand(); // 0x520ecd
                        p.scale = (r as f32 * 0.02) / 32767.0 + 0.1;
                    }
                }
                if t == 4 || matches!(p.kind, 2..=4) {
                    zone.props.push(p);
                }
            } else {
                let a = zone.column(x, y).climate_a;
                if a <= 0.25 {
                    // Kinds 0x1b/0x1c (0x520a60).
                    if self.rng.rand() % 100 == 0 {
                        let mut p = prop_at(x, y, h);
                        let q = self.rng.rand() % 4; // 0x520b23
                        p.rotation = (q * 90) as f32;
                        p.scale = 0.075;
                        let r = self.rng.rand() % 2; // 0x520b51
                        p.kind = 0x1c - u32::from(r != 0);
                        zone.props.push(p);
                    }
                } else {
                    // Kinds 3/4/0xb/0xc (0x52066d).
                    let mut p = prop_at(x, y, h);
                    let r = self.rng.rand(); // 0x52071a
                    p.rotation = (r as f32 * 360.0) / 32767.0;
                    p.scale = 0.075;
                    let dx = f64::from(x) * 0.01;
                    let dy = f64::from(y) * 0.01;
                    let n = noise(dx + 9843.0, dy + 8437.0);
                    p.kind = if n <= 0.0 {
                        let n = noise(dx + 34234.0, dy + 234234.0);
                        u32::from(n <= 0.5) + 0xb
                    } else {
                        let n = noise(f64::from(fx * 0.01 + 34234.0), f64::from(fy * 0.01 + 234234.0));
                        u32::from(n > 0.0) + 3
                    };
                    if p.kind == 2 || p.kind == 3 {
                        p.flags |= 4;
                    }
                    if p.kind == 0xb {
                        let r = self.rng.rand(); // 0x520890
                        p.flags |= 4;
                        p.scale = (r as f32 * 0.05) / 32767.0 + 0.05;
                    }
                    if p.kind == 0xc {
                        let r = self.rng.rand(); // 0x5208cf
                        p.flags |= 4;
                        p.scale = (r as f32 * 0.02) / 32767.0 + 0.1;
                    }
                    if t == 4 || p.kind == 2 || p.kind == 3 {
                        zone.props.push(p);
                    }
                }
            }
        }

        // Ground items (0x00520931).
        if t == 4 {
            if self.rng.rand() % 150 == 0 {
                // 0x52093a
                let mut it = item_at(x, y, h);
                let r = self.rng.rand(); // 0x5209fc
                it.rotation = (r as f32 / 32767.0) * 360.0;
                let sub: u16 = match self.rng.rand() % 2 {
                    // 0x520a28
                    0 => 0xf,
                    _ => {
                        it.f134 = 0.1;
                        0x16
                    }
                };
                it.item.item_type = 0xb;
                it.item.sub_type = sub as u8;
                // +0x10 u16 = 1 (constructor default), +0x138 u8 = 2 (not tracked).
                zone.items.push(it);
            }
        } else if t == 0xa && self.rng.rand() % 300 == 0 {
            // 0x520f69
            let mut it = item_at(x, y, h);
            let r = self.rng.rand(); // 0x52102b
            it.rotation = (r as f32 / 32767.0) * 360.0;
            let _ = self.rng.rand(); // 0x521057: `% 2`, always < 2
            it.f134 = 0.1;
            it.item.item_type = 0xb;
            it.item.sub_type = 0x18;
            // +0x10 u16 = 1 (constructor default), +0x138 u8 = 2 (not tracked).
            zone.items.push(it);
        }
    }

    /// Kind 0x32/0x33 statics on a type-0xb block with air above (`0x0051fb2c..0x0051fcef`).
    fn decorate_static(&mut self, zone: &mut Zone, x: i32, y: i32, h: i32) {
        let f = self.cell_falloff(x, y);
        if f <= 0.75 || (y * 0x5a + x) % 470 != 0 {
            return;
        }
        if self.rng.rand() % 16 != 0 {
            // 0x51fb7e
            return;
        }
        for k in 0..7 {
            if solid(zone.block(x, y, h + k)[3] & 0x1f) {
                return;
            }
        }
        let b = zone.column(x, y).climate_b;
        let (kind, s) = if b <= 0.8 { (0x32, 2.0) } else { (0x33, 1.0) };
        let mut st = Static { kind, x: from_block(x), y: from_block(y), z: from_block(h), scale: [s, s, 8.0], ..Static::NEW };
        st.rotation = self.rng.rand() % 4; // 0x51fcb9
        zone.statics.push(st);
    }
}
