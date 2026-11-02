//! The screen reader: reads a 1920x1080 picture of the Tartarus Prison screen (see READER.md). Everything is measured
//! against templates and spots in `data/reader-1080p.json` and `data/layout-1080p.json`, built from recorded runs.

use crate::game::{Code, Inmate, CELLS, LEGENDARY, MAGIC, NORMAL, RARE, ULTIMATE};
use serde_json::Value;
use std::sync::OnceLock;

pub const WIDTH: usize = 1920;
pub const HEIGHT: usize = 1080;

/// A picture: width, height and RGB bytes, row by row.
#[derive(Clone)]
pub struct Frame {
    pub w: usize,
    pub h: usize,
    pub rgb: Vec<u8>,
}

impl Frame {
    pub fn px(&self, x: usize, y: usize) -> [f32; 3] {
        let i = (y * self.w + x) * 3;
        [
            self.rgb[i] as f32,
            self.rgb[i + 1] as f32,
            self.rgb[i + 2] as f32,
        ]
    }

    pub fn gray(&self, x: usize, y: usize) -> f32 {
        let [r, g, b] = self.px(x, y);
        r * 0.299 + g * 0.587 + b * 0.114
    }

    pub fn from_png(bytes: &[u8]) -> Result<Frame, String> {
        let dec = png::Decoder::new(std::io::Cursor::new(bytes));
        let mut r = dec.read_info().map_err(|e| e.to_string())?;
        let mut buf = vec![0; r.output_buffer_size()];
        let info = r.next_frame(&mut buf).map_err(|e| e.to_string())?;
        let (w, h) = (info.width as usize, info.height as usize);
        let rgb = match info.color_type {
            png::ColorType::Rgb => buf[..w * h * 3].to_vec(),
            png::ColorType::Rgba => buf[..w * h * 4]
                .chunks_exact(4)
                .flat_map(|p| [p[0], p[1], p[2]])
                .collect(),
            t => return Err(format!("unsupported PNG colour type {t:?}")),
        };
        Ok(Frame { w, h, rgb })
    }
}

fn hsv(p: [f32; 3]) -> (f32, f32, f32) {
    let (r, g, b) = (p[0] / 255.0, p[1] / 255.0, p[2] / 255.0);
    let mx = r.max(g).max(b);
    let mn = r.min(g).min(b);
    let d = mx - mn + 1e-6;
    let h = if mx == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if mx == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } * 60.0;
    let s = if mx > 0.0 {
        (mx - mn) / (mx + 1e-6)
    } else {
        0.0
    };
    (h, s, mx)
}

/// Grey levels of a region averaged in blocks, zero-mean and unit-length (so the dot product of two is their
/// correlation).
fn block_vector(
    f: &Frame,
    x0: usize,
    y0: usize,
    w: usize,
    h: usize,
    bw: usize,
    bh: usize,
) -> Vec<f32> {
    let (nx, ny) = (w / bw, h / bh);
    let mut v = Vec::with_capacity(nx * ny);
    for by in 0..ny {
        for bx in 0..nx {
            let mut s = 0.0;
            for y in 0..bh {
                for x in 0..bw {
                    s += f.gray(x0 + bx * bw + x, y0 + by * bh + y);
                }
            }
            v.push(s / (bw * bh) as f32);
        }
    }
    normalize(v)
}

fn normalize(mut v: Vec<f32>) -> Vec<f32> {
    let m = v.iter().sum::<f32>() / v.len() as f32;
    v.iter_mut().for_each(|x| *x -= m);
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        v.iter_mut().for_each(|x| *x /= n);
    }
    v
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

struct Data {
    cells: [(usize, usize); CELLS],
    codes: Vec<Vec<f32>>,
    label_destination: Vec<f32>,
    label_decision: Vec<f32>,
    label_any_cell: Vec<f32>,
    label: [usize; 4],
    office: Vec<f32>,
    office_region: [usize; 4],
    titles: Vec<(String, Vec<f32>)>,
    title_slots: Vec<usize>,
    floor_base: [f32; CELLS],
    arrow_spots: [(f32, f32); CELLS],
    chromatic_slots: Vec<(usize, usize)>,
    objective_region: [usize; 4],
    objectives: Vec<(String, Vec<f32>)>,
    rumor_region: [usize; 4],
    rumors: Vec<(String, Vec<f32>)>,
    /// "N day(s) until Riot" at the top of the board, by N.
    days_left: Vec<(u32, Vec<f32>)>,
    incident_cards: Vec<(String, Vec<f32>)>,
}

fn data() -> &'static Data {
    static D: OnceLock<Data> = OnceLock::new();
    D.get_or_init(|| {
        let r: Value = serde_json::from_str(include_str!("../data/reader-1080p.json"))
            .expect("reader-1080p.json");
        let l: Value = serde_json::from_str(include_str!("../data/layout-1080p.json"))
            .expect("layout-1080p.json");
        let vecf = |v: &Value| -> Vec<f32> {
            v.as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_f64().unwrap() as f32)
                .collect()
        };
        let quad = |v: &Value| -> [usize; 4] {
            let a: Vec<usize> = v
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_u64().unwrap() as usize)
                .collect();
            [a[0], a[1], a[2], a[3]]
        };
        let mut cells = [(0, 0); CELLS];
        let mut floor_base = [0.11; CELLS];
        let mut arrow_spots = [(0.0, 0.0); CELLS];
        for n in 1..=CELLS {
            let k = n.to_string();
            let c = &l["cells"][&k];
            cells[n - 1] = (
                c[0].as_u64().unwrap() as usize,
                c[1].as_u64().unwrap() as usize,
            );
            if let Some(b) = r["floor_base"][&k].as_f64() {
                floor_base[n - 1] = b as f32;
            }
            let a = &r["arrow_spots"][&k];
            arrow_spots[n - 1] = (a[0].as_f64().unwrap() as f32, a[1].as_f64().unwrap() as f32);
        }
        let mut titles: Vec<(String, Vec<f32>)> = r["titles"]
            .as_object()
            .unwrap()
            .iter()
            // "Name#2": another look of the same title (a selected card is brighter)
            .map(|(k, v)| (k.split('#').next().unwrap().to_string(), vecf(v)))
            .collect();
        titles.sort_by(|a, b| a.0.cmp(&b.0));
        Data {
            cells,
            codes: r["codes"].as_array().unwrap().iter().map(vecf).collect(),
            label_destination: vecf(&r["label_destination"]),
            label_decision: vecf(&r["label_decision"]),
            label_any_cell: vecf(&r["label_any_cell"]),
            label: quad(&r["label_region"]),
            office: vecf(&r["office"]),
            office_region: quad(&r["office_region"]),
            titles,
            title_slots: r["title_slots"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x.as_u64().unwrap() as usize)
                .collect(),
            floor_base,
            arrow_spots,
            objective_region: quad(&r["objective_region"]),
            objectives: r["objectives"]
                .as_object()
                .map(|o| o.iter().map(|(k, v)| (k.clone(), vecf(v))).collect())
                .unwrap_or_default(),
            rumor_region: if r["rumor_region"].is_array() {
                quad(&r["rumor_region"])
            } else {
                [680, 318, 1240, 372]
            },
            // a name may come back as "name#2": another picture of the same card (it fades in)
            days_left: r["days_left"]
                .as_object()
                .map(|o| {
                    o.iter()
                        .filter_map(|(k, v)| Some((k.parse().ok()?, vecf(v))))
                        .collect()
                })
                .unwrap_or_default(),
            rumors: r["rumors"]
                .as_object()
                .map(|o| {
                    o.iter()
                        .map(|(k, v)| (k.split('#').next().unwrap_or(k).to_string(), vecf(v)))
                        .collect()
                })
                .unwrap_or_default(),
            // each card's title, several pictures each (it fades in)
            incident_cards: r["incident_cards"]
                .as_object()
                .map(|o| {
                    o.iter()
                        .flat_map(|(k, v)| {
                            v.as_array()
                                .into_iter()
                                .flatten()
                                .map(move |x| (k.clone(), vecf(x)))
                        })
                        .collect()
                })
                .unwrap_or_default(),
            chromatic_slots: r["chromatic_slots"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| {
                    (
                        p[0].as_u64().unwrap() as usize,
                        p[1].as_u64().unwrap() as usize,
                    )
                })
                .collect(),
        }
    })
}

/// A cell as read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CellRead {
    Locked,
    Empty,
    Office,
    Inmate {
        code: Code,
        rarity: u8,
        code_score: f32,
    },
}

/// The bottom label of the board screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Label {
    Destination,
    Decision,
    /// "Select an Objective": move to any cell (Sewer Ratway).
    AnyCell,
}

#[derive(Clone, Debug)]
pub struct BoardRead {
    /// How much the Interrogation Room's picture matches: the board is on screen above about 0.6.
    pub office_match: f32,
    pub label: Option<Label>,
    /// The red tint of the Consume target screen.
    pub red: f32,
    pub cells: [CellRead; CELLS],
    /// The warden's cell (0-based), from the gold arrow.
    pub warden: Option<usize>,
    pub chromatics: u8,
    /// How lit each cell's floor is above its usual (the destinations offered light up).
    pub lit: [f32; CELLS],
    /// The Interrogation Room lit up: offered as a destination.
    pub office_lit: bool,
    /// The cells with a Prison Incident's "!" beside them (locked or not).
    pub incidents: [bool; CELLS],
    /// Locked cells showing a "?" portrait: the rarity behind the door (its glow).
    pub revealed: [Option<u8>; CELLS],
}

impl BoardRead {
    pub fn on_board(&self) -> bool {
        self.office_match > 0.6
    }

    /// The `k` destinations offered: the most lit cells among the 6 ahead of the warden.
    pub fn destinations(&self, k: usize) -> Vec<usize> {
        let Some(w) = self.warden else {
            return Vec::new();
        };
        let mut ahead: Vec<(f32, usize)> = (1..=6)
            .map(|d| (w + d) % CELLS)
            .map(|c| {
                (
                    if c == 0 {
                        if self.office_lit {
                            1.0
                        } else {
                            -1.0
                        }
                    } else {
                        self.lit[c]
                    },
                    c,
                )
            })
            .collect();
        ahead.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
        let mut out: Vec<usize> = ahead
            .into_iter()
            .take(k)
            .filter(|(v, _)| *v > 0.03)
            .map(|(_, c)| c)
            .collect();
        out.sort_by_key(|&c| (c + CELLS - w) % CELLS);
        out
    }
}

fn region_mean(f: &Frame, x0: usize, y0: usize, x1: usize, y1: usize) -> f32 {
    let mut s = 0.0;
    for y in y0..y1 {
        for x in x0..x1 {
            let p = f.px(x, y);
            s += p[0] + p[1] + p[2];
        }
    }
    s / (3.0 * ((x1 - x0) * (y1 - y0)) as f32)
}

fn read_cell(f: &Frame, n: usize) -> CellRead {
    let d = data();
    if n == 0 {
        return CellRead::Office;
    }
    let (x, y) = d.cells[n];
    // the padlock under the portrait spot
    let mut gold = 0usize;
    let mut tot = 0usize;
    for yy in y + 45..y + 70 {
        for xx in x - 12..x + 20 {
            let (h, s, v) = hsv(f.px(xx, yy));
            tot += 1;
            if v > 0.6 && s > 0.15 && s < 0.6 && h > 20.0 && h < 50.0 {
                gold += 1;
            }
        }
    }
    if gold as f32 / tot as f32 > 0.05 {
        return CellRead::Locked;
    }
    // a portrait has much more contrast than an empty floor
    let mut vals = Vec::with_capacity(50 * 50 * 3);
    for yy in y - 30..y + 20 {
        for xx in x - 25..x + 25 {
            vals.extend_from_slice(&f.px(xx, yy));
        }
    }
    let m = vals.iter().sum::<f32>() / vals.len() as f32;
    let sd =
        (vals.iter().map(|v| (v - m) * (v - m)).sum::<f32>() / vals.len() as f32).sqrt() / 255.0;
    if sd < 0.13 {
        return CellRead::Empty;
    }
    let (bx, by) = (x - 52, y - 62);
    let face = block_vector(f, bx + 28, by + 40, 48, 51, 3, 3);
    let mut scores: Vec<(f32, usize)> = d
        .codes
        .iter()
        .enumerate()
        .map(|(k, t)| (dot(&face, t), k))
        .collect();
    scores.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    let code = [Code::Elephant, Code::Lion, Code::Wolf, Code::Rat][scores[0].1];
    CellRead::Inmate {
        code,
        rarity: read_rarity(f, bx, by),
        code_score: scores[0].0,
    }
}

/// The glow's colour in two bands beside the portrait: white (Normal), blue, purple, orange, red.
fn read_rarity(f: &Frame, bx: usize, by: usize) -> u8 {
    let mut px: Vec<(f32, f32, f32)> = Vec::new();
    for yy in by + 35..by + 95 {
        for xx in (bx + 12..bx + 22).chain(bx + 82..bx + 92) {
            px.push(hsv(f.px(xx, yy)));
        }
    }
    // the warden's cell is lit gold (hue ~30, saturation ~0.35): that's the floor, not a Legendary glow (~0.8)
    px.retain(|&(h, s, _)| !((15.0..55.0).contains(&h) && s < 0.55));
    if px.len() < 60 {
        return NORMAL;
    }
    px.sort_by(|a, b| (b.1 * b.2).partial_cmp(&(a.1 * a.2)).unwrap());
    let top = &px[..px.len() / 3];
    let (mut cx, mut cy, mut sm) = (0.0f32, 0.0f32, 0.0f32);
    for &(h, s, _) in top {
        cx += h.to_radians().cos() * s;
        cy += h.to_radians().sin() * s;
        sm += s;
    }
    let s = sm / top.len() as f32;
    let h = cy.atan2(cx).to_degrees().rem_euclid(360.0);
    if s < 0.3 {
        NORMAL
    } else if (185.0..240.0).contains(&h) {
        MAGIC
    } else if (240.0..300.0).contains(&h) {
        RARE
    } else if (12.0..60.0).contains(&h) {
        LEGENDARY
    } else if !(60.0..=335.0).contains(&h) {
        ULTIMATE
    } else {
        NORMAL
    }
}

/// The gold arrow: the biggest gold blob on the board that isn't a portrait, its glow or a `!`.
fn read_warden(f: &Frame) -> Option<usize> {
    let d = data();
    let (x0, y0, x1, y1) = (420usize, 60usize, 1500usize, 700usize);
    let (w, h) = (x1 - x0, y1 - y0);
    let mut m = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            let (hh, s, v) = hsv(f.px(x0 + x, y0 + y));
            m[y * w + x] = hh > 35.0 && hh < 58.0 && s > 0.5 && v > 0.7;
        }
    }
    let mut clear = |xa: i64, ya: i64, xb: i64, yb: i64| {
        for y in ya.max(0)..yb.min(h as i64) {
            for x in xa.max(0)..xb.min(w as i64) {
                m[y as usize * w + x as usize] = false;
            }
        }
    };
    for &(cx, cy) in &d.cells {
        let (cx, cy) = (cx as i64 - x0 as i64, cy as i64 - y0 as i64);
        clear(cx - 62, cy - 75, cx + 62, cy + 75);
    }
    // connected blobs
    let mut seen = vec![false; w * h];
    let mut best: (usize, f32, f32) = (0, 0.0, 0.0);
    let mut stack = Vec::new();
    for start in 0..w * h {
        if !m[start] || seen[start] {
            continue;
        }
        let (mut n, mut sx, mut sy) = (0usize, 0.0f32, 0.0f32);
        let (mut bx0, mut bx1, mut by0, mut by1) = (usize::MAX, 0usize, usize::MAX, 0usize);
        stack.push(start);
        seen[start] = true;
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            n += 1;
            bx0 = bx0.min(x);
            bx1 = bx1.max(x);
            by0 = by0.min(y);
            by1 = by1.max(y);
            sx += x as f32;
            sy += y as f32;
            for (dx, dy) in [
                (1i64, 0i64),
                (-1, 0),
                (0, 1),
                (0, -1),
                (1, 1),
                (1, -1),
                (-1, 1),
                (-1, -1),
            ] {
                let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                if nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h {
                    let j = ny as usize * w + nx as usize;
                    if m[j] && !seen[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        // a Prison Incident's "!" is a narrow upright mark: not the arrow
        let (bw, bh) = (bx1 - bx0 + 1, by1 - by0 + 1);
        if bw <= 12 && bh >= 2 * bw {
            continue;
        }
        if n > best.0 {
            best = (n, sx / n as f32 + x0 as f32, sy / n as f32 + y0 as f32);
        }
    }
    if std::env::var("WARDEN_DEBUG").is_ok() {
        eprintln!("arrow blob {:?}", best);
    }
    if best.0 < 40 {
        return None;
    }
    let (k, dist) = d
        .arrow_spots
        .iter()
        .enumerate()
        .map(|(k, &(ax, ay))| (k, ((ax - best.1).powi(2) + (ay - best.2).powi(2)).sqrt()))
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
        .unwrap();
    (dist < 45.0).then_some(k)
}

pub fn read_board(f: &Frame) -> BoardRead {
    let d = data();
    let [ox0, oy0, ox1, oy1] = d.office_region;
    let office_match = dot(
        &block_vector(f, ox0, oy0, ox1 - ox0, oy1 - oy0, 5, 5),
        &d.office,
    );
    let [lx0, ly0, lx1, ly1] = d.label;
    let lv = block_vector(f, lx0, ly0, lx1 - lx0, ly1 - ly0, 4, 2);
    let label = if dot(&lv, &d.label_destination) > 0.8 {
        Some(Label::Destination)
    } else if dot(&lv, &d.label_decision) > 0.8 {
        Some(Label::Decision)
    } else if !d.label_any_cell.is_empty() && dot(&lv, &d.label_any_cell) > 0.8 {
        Some(Label::AnyCell)
    } else {
        None
    };
    let mut red = 0.0;
    for y in 900..1000 {
        for x in 100..400 {
            let p = f.px(x, y);
            red += p[0] - p[1];
        }
    }
    red /= 100.0 * 300.0;
    // the Consume screen's tint is red, not the orange glow of other screens (green above blue there)
    let mut gb = 0.0;
    for y in 900..1000 {
        for x in 100..400 {
            let p = f.px(x, y);
            gb += p[1] - p[2];
        }
    }
    if gb / (100.0 * 300.0) > 4.0 {
        red = 0.0;
    }
    // the label under the board sometimes isn't shown (after a Cancel, while it fades in): tell the screen by the
    // cards at the bottom instead, destination cards having a busy riveted top edge, the decision buttons a plain
    // one (measured on 27 pictures: destination cards 5.2 and up, buttons 2.8 to 3.8, nothing below 1)
    let label = match label {
        None if red <= 7.0 => {
            let mut t = 0.0;
            for y in 752..770 {
                for x in 640..1279 {
                    t += (f.gray(x + 1, y) - f.gray(x, y)).abs();
                }
            }
            let t = t / (18.0 * 639.0);
            // (destination cards without their label are still sliding in: the lit cells aren't shown yet)
            if t > 4.6 {
                None
            } else if t > 2.2 {
                Some(Label::Decision)
            } else {
                None
            }
        }
        l => l,
    };
    let mut cells = [CellRead::Locked; CELLS];
    let mut lit = [0.0; CELLS];
    for n in 0..CELLS {
        cells[n] = read_cell(f, n);
        if n > 0 {
            let (x, y) = d.cells[n];
            let fl = (region_mean(f, x - 75, y - 10, x - 58, y + 40)
                + region_mean(f, x + 58, y - 10, x + 75, y + 40))
                / 2.0
                / 255.0;
            lit[n] = fl - d.floor_base[n];
        }
    }
    let office_lit = region_mean(f, 470, 95, 575, 180) / 255.0 > 0.31;
    // a Chromatic's portrait is busy (face, bars, flames) where an empty slot is smooth, however lit or dimmed the
    // column is (measured: portraits 3.2 and up, empty slots 1.5 and less, on any screen)
    let chromatics = d
        .chromatic_slots
        .iter()
        .filter(|&&(x, y)| {
            let mut t = 0.0;
            for yy in y - 25..y + 25 {
                for xx in x - 22..x + 21 {
                    t += (f.gray(xx + 1, yy) - f.gray(xx, yy)).abs();
                }
            }
            t / (50.0 * 43.0) / 255.0 * 100.0 > 2.2
        })
        .count() as u8;
    let mut incidents = [false; CELLS];
    let mut revealed = [None; CELLS];
    for n in 1..CELLS {
        incidents[n] = incident_mark(f, d.cells[n]);
        if cells[n] == CellRead::Locked {
            revealed[n] = revealed_door(f, d.cells[n]);
        }
    }
    BoardRead {
        office_match,
        label,
        red,
        cells,
        warden: read_warden(f),
        chromatics,
        lit,
        office_lit,
        incidents,
        revealed,
    }
}

/// A locked cell with a "?" portrait shows the rarity behind its door: the portrait has contrast where a plain
/// locked cell is dark floor (measured: "?" doors 0.15 and up, plain ones 0.03 and less).
fn revealed_door(f: &Frame, (x, y): (usize, usize)) -> Option<u8> {
    let mut vals = Vec::with_capacity(50 * 50 * 3);
    for yy in y - 30..y + 20 {
        for xx in x - 25..x + 25 {
            vals.extend_from_slice(&f.px(xx, yy));
        }
    }
    let m = vals.iter().sum::<f32>() / vals.len() as f32;
    let sd =
        (vals.iter().map(|v| (v - m) * (v - m)).sum::<f32>() / vals.len() as f32).sqrt() / 255.0;
    if sd <= 0.1 {
        return None;
    }
    // a "?" portrait: a dark inside within a lit frame; not the light arrows of a destination, nor a Chromatic
    // leaving its cell (bright face)
    let v = |xx: usize, yy: usize| hsv(f.px(xx, yy)).2;
    let (mut inner, mut n) = (0.0, 0.0);
    for yy in y - 22..y + 12 {
        for xx in x - 18..x + 18 {
            inner += v(xx, yy);
            n += 1.0;
        }
    }
    let inner = inner / n;
    let (mut bright, mut n) = (0.0, 0.0);
    for yy in y - 35..y + 25 {
        for xx in x - 30..x + 30 {
            bright += (v(xx, yy) > 0.55) as u8 as f32;
            n += 1.0;
        }
    }
    let bright = bright / n;
    let (mut frame, mut n) = (0.0, 0.0);
    for yy in y - 25..y + 15 {
        for xx in (x - 31..x - 26).chain(x + 26..x + 31) {
            frame += v(xx, yy);
            n += 1.0;
        }
    }
    let frame = frame / n;
    (inner < 0.25 && (0.07..0.25).contains(&bright) && frame > 0.28)
        .then(|| read_rarity(f, x - 52, y - 62))
}

/// A Prison Incident's "!": a small tall bright orange mark right of the cell's centre (+62..80, +8..34), with
/// nothing orange around it (measured on 215 recorded frames: about 50 pixels inside, none around).
fn incident_mark(f: &Frame, (x, y): (usize, usize)) -> bool {
    let orange = |xx: usize, yy: usize| {
        let (h, s, v) = hsv(f.px(xx, yy));
        h > 28.0 && h < 55.0 && s > 0.55 && v > 0.8
    };
    let (mut inner, mut ring) = (0, 0);
    for yy in y - 2..y + 44 {
        for xx in x + 50..x + 92 {
            if orange(xx, yy) {
                if (y + 8..y + 34).contains(&yy) && (x + 62..x + 80).contains(&xx) {
                    inner += 1;
                } else {
                    ring += 1;
                }
            }
        }
    }
    inner >= 30 && ring * 2 <= inner
}

/// The instrument cards' names (by slot) and how well each matched; unknown names come back as None.
pub fn read_instrument_offer(f: &Frame) -> Vec<Option<(String, f32)>> {
    let d = data();
    d.title_slots
        .iter()
        .map(|&x0| {
            // only a title that's there: blank cards (still fading in) match anything faintly
            let mut g: Vec<f32> = Vec::with_capacity(410 * 50);
            for y in 470..520 {
                for x in x0 + 20..x0 + 430 {
                    g.push(f.gray(x, y));
                }
            }
            g.sort_by(|a, b| a.partial_cmp(b).unwrap());
            if g[g.len() * 99 / 100] < 100.0 {
                return None;
            }
            let mut best: (f32, &str) = (-1.0, "");
            for ty in (440..522).step_by(2) {
                for dx in [-4i32, 0, 4] {
                    let xs = (x0 as i32 + 20 + dx) as usize;
                    let v = block_vector(f, xs, ty - 22, 410, 44, 2, 2);
                    for (name, t) in &d.titles {
                        let s = dot(&v, t);
                        if s > best.0 {
                            best = (s, name);
                        }
                    }
                }
            }
            (best.0 > 0.75).then(|| (best.1.to_string(), best.0))
        })
        .collect()
}

/// The Warden's Treasure screen's header ("Prisoner rarity reached Chromatic. Warden's Treasure obtained."): light text
/// both left of and across the middle of the top line, where the instrument offer's shorter header has none on the
/// left (measured: treasure 0.25 and up on both sides, every other screen under 0.15 on one of them).
pub fn treasure_header(f: &Frame) -> bool {
    let share = |x0: usize, x1: usize| {
        let (mut b, mut n) = (0.0, 0.0);
        for y in 95..122 {
            for x in x0..x1 {
                b += (f.gray(x, y) > 120.0) as u8 as f32;
                n += 1.0;
            }
        }
        b / n
    };
    share(600, 780) > 0.2 && share(820, 1100) > 0.2
}

/// The label left of the top-right eye ("View Prison Status" / "Return to Selection"), shown over the Warden's Treasure
/// and its board view (measured: 0.08-0.10 of the area lit there, a plain board about 0.01).
pub fn eye_label(f: &Frame) -> bool {
    let (mut b, mut n) = (0.0, 0.0);
    for y in 120..155 {
        for x in 1575..1795 {
            b += (f.gray(x, y) > 75.0) as u8 as f32;
            n += 1.0;
        }
    }
    b / n > 0.05
}

/// Which instrument card is selected (its orange frame), if any.
pub fn selected_instrument_slot(f: &Frame) -> Option<usize> {
    let d = data();
    let mut best = (0.0f32, None);
    for (k, &x0) in d.title_slots.iter().enumerate() {
        let mut c = 0;
        let mut t = 0;
        for y in 250..800 {
            for x in (x0 - 10..x0 + 25).chain(x0 + 420..x0 + 455) {
                let (h, s, v) = hsv(f.px(x, y));
                t += 1;
                if h > 12.0 && h < 40.0 && s > 0.5 && v > 0.45 {
                    c += 1;
                }
            }
        }
        let r = c as f32 / t as f32;
        if r > best.0 {
            best = (r, Some(k));
        }
    }
    if best.0 > 0.03 {
        best.1
    } else {
        None
    }
}

/// The instrument names the reader knows on cards.
pub fn known_titles() -> Vec<String> {
    let mut t: Vec<String> = data().titles.iter().map(|t| t.0.clone()).collect();
    t.dedup();
    t
}

/// A board read as a solver state's cells: unread or uncertain cells are reported in the second value.
pub fn to_cells(b: &BoardRead) -> ([crate::game::Cell; CELLS], Vec<String>) {
    use crate::game::Cell;
    let mut cells = [Cell::Locked; CELLS];
    let mut notes = Vec::new();
    for (n, c) in b.cells.iter().enumerate() {
        cells[n] = match *c {
            CellRead::Locked => Cell::Locked,
            CellRead::Empty => Cell::Empty,
            CellRead::Office => Cell::Office,
            CellRead::Inmate {
                code,
                rarity,
                code_score,
            } => {
                if code_score < 0.55 {
                    notes.push(format!(
                        "cell {}: not sure it's a {} ({:.2})",
                        n + 1,
                        code.name(),
                        code_score
                    ));
                }
                Cell::Inmate(Inmate { code, rarity })
            }
        };
    }
    (cells, notes)
}

/// The objective card's title ("prison-walk", "undercurrents"), when it's one the reader knows.
pub fn read_objective(f: &Frame) -> Option<String> {
    let d = data();
    let [x0, y0, x1, y1] = d.objective_region;
    let v = block_vector(f, x0, y0, x1 - x0, y1 - y0, 4, 2);
    d.objectives
        .iter()
        .map(|(k, t)| (dot(&v, t), k))
        .filter(|(s, _)| *s > 0.85)
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap())
        .map(|(_, k)| k.clone())
}

/// The title strip of an instrument card (slot 0-2) as the reader compares it, at the row where the title's text is
/// brightest: for learning a new card's name.
pub fn learn_title(f: &Frame, slot: usize) -> (usize, Vec<f32>) {
    let x0 = data().title_slots[slot];
    let ty = (440..522)
        .max_by_key(|&ty| {
            let mut c = 0;
            for y in ty - 6..ty + 6 {
                for x in x0 + 60..x0 + 390 {
                    if f.gray(x, y) > 150.0 {
                        c += 1;
                    }
                }
            }
            c
        })
        .unwrap();
    (ty, block_vector(f, x0 + 20, ty - 22, 410, 44, 2, 2))
}

/// The red jaws around the cells the eater can Consume (on the target screen): bright red in the bands either side of
/// the portrait. An Ultimate's red glow stays inside them (measured on 7 target screens; jaws give 1-3%, others 0).
pub fn read_jaws(f: &Frame) -> [bool; CELLS] {
    let d = data();
    let mut out = [false; CELLS];
    for n in 1..CELLS {
        let (x, y) = d.cells[n];
        let (mut c, mut t) = (0, 0);
        for yy in y - 70..y + 70 {
            for xx in (x - 75..x - 48).chain(x + 48..x + 75) {
                let (h, s, v) = hsv(f.px(xx, yy));
                t += 1;
                if !(15.0..=345.0).contains(&h) && s > 0.6 && v > 0.6 {
                    c += 1;
                }
            }
        }
        out[n] = c as f32 / t as f32 > 0.006;
    }
    out
}

/// The label under the board as the reader compares it: for learning a new label.
pub fn label_vector(f: &Frame) -> Vec<f32> {
    let [lx0, ly0, lx1, ly1] = data().label;
    block_vector(f, lx0, ly0, lx1 - lx0, ly1 - ly0, 4, 2)
}

/// The objective card's title as the reader compares it: for learning a new objective.
pub fn objective_vector(f: &Frame) -> Vec<f32> {
    let [x0, y0, x1, y1] = data().objective_region;
    block_vector(f, x0, y0, x1 - x0, y1 - y0, 4, 2)
}

/// The top line "N day(s) until Riot" as a vector (its number and the words, which shift with the number's width).
pub fn days_vector(f: &Frame) -> Vec<f32> {
    block_vector(f, 780, 8, 220, 44, 2, 2)
}

/// The days until the riot, read from the top of the board (for computers whose game log doesn't tell the day).
pub fn read_days_left(f: &Frame) -> Option<u32> {
    let v = days_vector(f);
    let mut best = (0.0f32, None);
    for (n, t) in &data().days_left {
        let s = dot(&v, t);
        if s > best.0 {
            best = (s, Some(*n));
        }
    }
    if best.0 > 0.92 {
        best.1
    } else {
        None
    }
}

/// The Rumor card's title as the reader compares it: for learning a new Rumor.
pub fn rumor_vector(f: &Frame) -> Vec<f32> {
    let [x0, y0, x1, y1] = data().rumor_region;
    block_vector(f, x0, y0, x1 - x0, y1 - y0, 4, 2)
}

/// The Rumor card's title ("cage-fight", ...), when it's one the reader knows.
pub fn read_rumor(f: &Frame) -> Option<String> {
    // only once the card is fully shown: its title is bright then (99th percentile ~190, ~155 while fading in)
    let [x0, y0, x1, y1] = data().rumor_region;
    let mut g: Vec<f32> = Vec::with_capacity((x1 - x0) * (y1 - y0));
    for y in y0..y1 {
        for x in x0..x1 {
            g.push(f.gray(x, y));
        }
    }
    g.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if g[g.len() * 99 / 100] < 175.0 {
        return None;
    }
    let v = rumor_vector(f);
    data()
        .rumors
        .iter()
        .map(|(k, t)| (dot(&v, t), k))
        .filter(|(s, _)| *s > 0.85)
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap())
        .map(|(_, k)| k.clone())
}

const INCIDENT_TITLE: [usize; 4] = [1110, 205, 1770, 262];

/// The Prison Incident card's title as the reader compares it: for learning a new card.
pub fn incident_vector(f: &Frame) -> Vec<f32> {
    let [x0, y0, x1, y1] = INCIDENT_TITLE;
    block_vector(f, x0, y0, x1 - x0, y1 - y0, 4, 2)
}

/// Which Prison Incident card this is ("Usurpation II", "Chain Reaction", ...), when the reader knows it.
pub fn read_incident(f: &Frame) -> Option<String> {
    let v = incident_vector(f);
    data()
        .incident_cards
        .iter()
        .map(|(k, t)| (dot(&v, t), k))
        .filter(|(s, _)| *s > 0.8)
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap())
        .map(|(_, k)| k.clone())
}

/// The Interrogation Room's picture region, and how much a picture matches it there (the board is on screen above
/// about 0.6): for finding the scale of a picture of another size.
pub fn office_region() -> [usize; 4] {
    data().office_region
}

pub fn office_match(f: &Frame) -> f32 {
    let d = data();
    let [ox0, oy0, ox1, oy1] = d.office_region;
    dot(
        &block_vector(f, ox0, oy0, ox1 - ox0, oy1 - oy0, 5, 5),
        &d.office,
    )
}
