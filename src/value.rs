//! A learned worth of a board: the Chromatics a run is expected to end with from here, from features of the board
//! (rarities by Code, food for the next bite, doors, days, instruments). Trained on simulated runs
//! (examples/value_data.rs, tools/train_value.py); the weights are in data/value.json.
use crate::game::*;
use crate::instruments::{Instrument, ALL};

pub const N_FEATURES: usize = 4 * 5 + 5 + 5 + 8 + ALL.len() + 29;

/// The board as numbers for the model.
pub fn features(s: &State) -> Vec<f32> {
    let mut f = Feats::default();
    fill(s, &mut f);
    f.v[..f.n].to_vec()
}

/// The features without allocating (the solver asks for them at every step of every future).
struct Feats {
    v: [f32; N_FEATURES],
    n: usize,
}

impl Default for Feats {
    fn default() -> Self {
        Feats { v: [0.0; N_FEATURES], n: 0 }
    }
}

impl Feats {
    #[inline]
    fn push(&mut self, x: f32) {
        self.v[self.n] = x;
        self.n += 1;
    }
    #[inline]
    fn extend_from_slice(&mut self, xs: &[f32]) {
        for &x in xs {
            self.push(x);
        }
    }
}

fn fill(s: &State, f: &mut Feats) {
    // the Inmates once (cells in order)
    let mut inm = [0usize; CELLS];
    let mut ni = 0;
    for i in 0..CELLS {
        if s.inmate(i).is_some() {
            inm[ni] = i;
            ni += 1;
        }
    }
    let inmates = &inm[..ni];
    let mut fed = [false; CELLS];
    for &i in inmates {
        fed[i] = s.has_target(i);
    }
    // Inmates by Code and rarity
    let mut grid = [[0f32; 5]; 4];
    for &i in inmates {
        let m = s.inmate(i).unwrap();
        let c = Code::ALL.iter().position(|&x| x == m.code).unwrap();
        grid[c][(m.rarity as usize).min(4)] += 1.0;
    }
    for row in &grid {
        f.extend_from_slice(row);
    }
    // per rarity: how many different Codes (food for chains), and Inmates with something to eat
    for r in 0..5 {
        f.push((0..4).filter(|&c| grid[c][r] > 0.0).count() as f32);
    }
    let mut eaters = [0f32; 5];
    for &i in inmates {
        if fed[i] {
            eaters[(s.inmate(i).unwrap().rarity as usize).min(4)] += 1.0;
        }
    }
    f.extend_from_slice(&eaters);
    // trained on 20-day runs: the extra days of an Iron Law run would be outside what it learned
    static RAW: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let raw = *RAW.get_or_init(|| std::env::var("VDAYS_RAW").is_ok());
    let days = if raw { s.days_left as f32 } else { (s.days_left as f32).min(20.0) };
    let chroma = s.chromatics as f32;
    let locked = (0..CELLS).filter(|&i| s.cells[i] == Cell::Locked).count() as f32;
    let n = ni as f32;
    f.push(days);
    f.push(days * days / 20.0);
    f.push(chroma);
    f.push(locked);
    f.push(n);
    f.push(s.picks as f32 + s.offers as f32);
    f.push(if s.setup.opulence { 1.0 } else { 0.0 });
    f.push(s.incidents.count_ones() as f32);
    for i in ALL {
        f.push(if s.has(i) { 1.0 } else { 0.0 });
    }
    // marks on the board
    f.push(s.hunt.map_or(0.0, |h| h.mornings as f32));
    f.push(s.hunt.and_then(|h| s.inmate(h.lion)).map_or(0.0, |m| m.rarity as f32));
    f.push(s.tusks.len() as f32);
    f.push(s.scratches.len() as f32);
    f.push(s.nests.len() as f32);
    f.push(s.nests.iter().filter(|n| n.1 <= 1).count() as f32);
    // "?" doors by the rarity shown
    let mut q = [0f32; 5];
    for &(_, r) in &s.revealed {
        q[(r as usize).min(4)] += 1.0;
    }
    f.extend_from_slice(&q);
    // the objective and how far along it is
    let (kind, part) = match s.objective {
        None => (0, 1.0),
        Some(Objective::PrisonWalk { walked }) => (1, walked as f32 / PRISON_WALK_TILES as f32),
        Some(Objective::KingOfTheCaged) => (2, 0.0),
        Some(Objective::Undercurrents { triggered }) => (3, triggered as f32 / UNDERCURRENTS_INCIDENTS as f32),
        Some(Objective::BehindCellDoors { opened }) => (4, opened as f32 / BEHIND_CELL_DOORS_CELLS as f32),
        Some(Objective::SurvivalOfTheFittest { consumed }) => (5, consumed as f32 / SURVIVAL_CONSUMES as f32),
    };
    for k in 0..6 {
        f.push(if k == kind { 1.0 } else { 0.0 });
    }
    f.push(part);
    f.push(s.rerolls as f32);
    f.push(if s.setup.cage_fight { 1.0 } else { 0.0 });
    f.push(if s.setup.tyranny { 1.0 } else { 0.0 });
    // what's in reach of the warden's next walk (the 6 cells ahead): doors, Inmates with food, the office
    let ahead = |d: usize| (s.pos + d) % CELLS;
    f.push((1..=6).filter(|&d| s.cells[ahead(d)] == Cell::Locked).count() as f32);
    f.push((1..=6).filter(|&d| fed[ahead(d)]).count() as f32);
    f.push(if (1..=6).any(|d| ahead(d) == 0) { 1.0 } else { 0.0 });
    // neighbours: Inmates side by side, and locked cells next to Inmates
    let mut pairs = 0f32;
    let mut beside_doors = 0f32;
    for &i in inmates {
        let r = (i + 1) % CELLS;
        if s.inmate(r).is_some() {
            pairs += 1.0;
        }
        for j in [(i + CELLS - 1) % CELLS, (i + 1) % CELLS] {
            if s.cells[j] == Cell::Locked {
                beside_doors += 1.0;
            }
        }
    }
    f.push(pairs);
    f.push(beside_doors);
    // the best Inmate and how many of the top two rarities
    let top = inmates.iter().map(|&i| s.inmate(i).unwrap().rarity).max().unwrap_or(0);
    f.push(top as f32);
    f.push(inmates.iter().filter(|&&i| s.inmate(i).unwrap().rarity + 1 >= top && top > 0).count() as f32);
    f.push(s.bites_today as f32);
    debug_assert_eq!(f.n, N_FEATURES);
}

/// The n-tuple board value's tuples: 3 cells side by side, at each of the 16 places counted from the warden's cell, and
/// the days left with the Chromatics. The 7 places nearest ahead (within a walk's reach) have their own table, the
/// rest share one (8 tables).
pub const NT_TUPLES: usize = CELLS;
pub const NT_OWN: usize = 7;
/// Values a cell takes: locked, empty, the office, or an Inmate (4 Codes x 5 rarities).
pub const NT_CELL: usize = 23;
pub const NT_TABLE: usize = NT_CELL * NT_CELL * NT_CELL;
/// The days-and-Chromatics table: 21 x 11.
pub const NT_DAYS: usize = 21 * 11;

fn nt_cell(s: &State, i: usize) -> usize {
    match s.cells[i] {
        Cell::Locked => 0,
        Cell::Empty => 1,
        Cell::Office => 2,
        Cell::Inmate(m) => {
            let c = Code::ALL.iter().position(|&x| x == m.code).unwrap_or(0);
            3 + c * 5 + (m.rarity as usize).min(4)
        }
    }
}

/// The board as n-tuple indices: for each tuple its entry in its table (tables laid out one after the other), then the
/// days-and-Chromatics entry.
pub fn ntuple_indices(s: &State) -> [usize; NT_TUPLES + 1] {
    let mut out = [0usize; NT_TUPLES + 1];
    for d in 0..NT_TUPLES {
        let a = nt_cell(s, (s.pos + d) % CELLS);
        let b = nt_cell(s, (s.pos + d + 1) % CELLS);
        let c = nt_cell(s, (s.pos + d + 2) % CELLS);
        out[d] = d.min(NT_OWN) * NT_TABLE + (a * NT_CELL + b) * NT_CELL + c;
    }
    let days = (s.days_left as usize).min(20);
    out[NT_TUPLES] = (NT_OWN + 1) * NT_TABLE + days * 11 + (s.chromatics as usize).min(10);
    out
}

/// The n-tuple board value: a sum of table entries (the board, 16 lookups) and a linear term on the features (the
/// instruments, the objective, the marks...). About ten times cheaper than the network.
pub struct NTuple {
    tables: Vec<f32>,
    linear: Vec<f32>,
    bias: f32,
}

impl NTuple {
    pub fn from_json(v: &serde_json::Value) -> Option<NTuple> {
        let vecf = |x: &serde_json::Value| -> Option<Vec<f32>> {
            x.as_array()?.iter().map(|y| y.as_f64().map(|z| z as f32)).collect()
        };
        let tables = vecf(&v["tables"])?;
        let linear = vecf(&v["linear"])?;
        (tables.len() == (NT_OWN + 1) * NT_TABLE + NT_DAYS && linear.len() == N_FEATURES).then(|| NTuple {
            tables,
            linear,
            bias: v["bias"].as_f64().unwrap_or(0.0) as f32,
        })
    }

    pub fn eval(&self, s: &State) -> f64 {
        let mut out = self.bias;
        for i in ntuple_indices(s) {
            out += self.tables[i];
        }
        let mut f = Feats::default();
        fill(s, &mut f);
        for (x, w) in f.v.iter().zip(&self.linear) {
            out += x * w;
        }
        out as f64
    }
}

/// The n-tuple model, from data/ntuple.json (or env NTUPLE_JSON), when there's one.
pub fn ntuple() -> Option<&'static NTuple> {
    static M: std::sync::OnceLock<Option<NTuple>> = std::sync::OnceLock::new();
    M.get_or_init(|| {
        let p = std::env::var("NTUPLE_JSON").unwrap_or_else(|_| {
            concat!(env!("CARGO_MANIFEST_DIR"), "/data/ntuple.json").to_string()
        });
        let text = std::fs::read_to_string(p).ok()?;
        serde_json::from_str::<serde_json::Value>(&text).ok().and_then(|v| NTuple::from_json(&v))
    })
    .as_ref()
}

/// A small network (one hidden layer) over the features, loaded from data/value.json.
pub struct Model {
    /// The first layer with the features' normalisation folded in, by feature: hidden k = sum over features i of
    /// w1[i*H + k] * raw feature i, + b1[k] (most features are 0 on a given board, and are skipped).
    w1: Vec<f32>,
    hidden: usize,
    b1: Vec<f32>,
    w2: Vec<f32>,
    b2: f32,
}

impl Model {
    pub fn from_json(v: &serde_json::Value) -> Option<Model> {
        let vecf = |x: &serde_json::Value| -> Option<Vec<f32>> {
            x.as_array()?.iter().map(|y| y.as_f64().map(|z| z as f32)).collect()
        };
        let mean = vecf(&v["mean"])?;
        if mean.len() != N_FEATURES {
            return None;
        }
        let scale = vecf(&v["scale"])?;
        let rows = v["w1"].as_array()?.iter().map(vecf).collect::<Option<Vec<_>>>()?;
        let b1 = vecf(&v["b1"])?;
        if scale.len() != N_FEATURES || rows.iter().any(|r| r.len() != N_FEATURES) || b1.len() != rows.len() {
            return None;
        }
        let h = rows.len();
        if h > 128 {
            return None;
        }
        let mut w1 = vec![0f32; h * N_FEATURES];
        let mut b = Vec::with_capacity(h);
        for (k, (row, b0)) in rows.iter().zip(&b1).enumerate() {
            let mut shift = *b0;
            for i in 0..N_FEATURES {
                let w = row[i] / scale[i];
                w1[i * h + k] = w;
                shift -= w * mean[i];
            }
            b.push(shift);
        }
        Some(Model {
            w1,
            hidden: h,
            b1: b,
            w2: vecf(&v["w2"])?,
            b2: v["b2"].as_f64()? as f32,
        })
    }

    pub fn eval(&self, s: &State) -> f64 {
        let mut f = Feats::default();
        fill(s, &mut f);
        let h = self.hidden;
        let mut acc = [0f32; 128];
        let acc = &mut acc[..h.min(128)];
        acc.copy_from_slice(&self.b1[..acc.len()]);
        for (i, &x) in f.v.iter().enumerate() {
            if x != 0.0 {
                let col = &self.w1[i * h..i * h + acc.len()];
                for (a, w) in acc.iter_mut().zip(col) {
                    *a += x * w;
                }
            }
        }
        let mut out = self.b2;
        for (a, w) in acc.iter().zip(&self.w2) {
            out += a.max(0.0) * w;
        }
        out as f64
    }
}

/// The trained model, if data/value.json holds one.
pub fn model() -> Option<&'static Model> {
    static M: std::sync::OnceLock<Option<Model>> = std::sync::OnceLock::new();
    M.get_or_init(|| {
        // VALUE_JSON: another model's file, to compare models on the same runs
        let file = std::env::var("VALUE_JSON").ok().and_then(|p| std::fs::read_to_string(p).ok());
        let text = file.as_deref().unwrap_or(include_str!("../data/value.json"));
        serde_json::from_str::<serde_json::Value>(text)
            .ok()
            .and_then(|v| Model::from_json(&v))
    })
    .as_ref()
}

/// The chance of reaching the 10 Chromatics, from data/value10.json (same features).
pub fn model10() -> Option<&'static Model> {
    static M: std::sync::OnceLock<Option<Model>> = std::sync::OnceLock::new();
    M.get_or_init(|| {
        // VALUE10_JSON: another model's file, to compare models on the same runs
        let file = std::env::var("VALUE10_JSON").ok().and_then(|p| std::fs::read_to_string(p).ok());
        let text = file.as_deref().unwrap_or(include_str!("../data/value10.json"));
        serde_json::from_str::<serde_json::Value>(text)
            .ok()
            .and_then(|v| Model::from_json(&v))
    })
    .as_ref()
}

pub fn has_model() -> bool {
    model().is_some()
}

#[allow(dead_code)]
fn _unused(_: Instrument) {}
