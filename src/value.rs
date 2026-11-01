//! A learned worth of a board: the Chromatics a run is expected to end with from here, from features of the board
//! (rarities by Code, food for the next bite, doors, days, instruments). Trained on simulated runs
//! (examples/value_data.rs, tools/train_value.py); the weights are in data/value.json.
use crate::game::*;
use crate::instruments::{Instrument, ALL};

pub const N_FEATURES: usize = 4 * 5 + 5 + 5 + 8 + ALL.len();

/// The board as numbers for the model.
pub fn features(s: &State) -> Vec<f32> {
    let mut f = Vec::with_capacity(N_FEATURES);
    // Inmates by Code and rarity
    let mut grid = [[0f32; 5]; 4];
    for i in s.inmates() {
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
    for i in s.inmates() {
        if !s.targets(i).is_empty() {
            eaters[(s.inmate(i).unwrap().rarity as usize).min(4)] += 1.0;
        }
    }
    f.extend_from_slice(&eaters);
    let days = s.days_left as f32;
    let chroma = s.chromatics as f32;
    let locked = s.locked().len() as f32;
    let n = s.inmates().len() as f32;
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
    f
}

/// A small network (one hidden layer) over the features, loaded from data/value.json.
pub struct Model {
    mean: Vec<f32>,
    scale: Vec<f32>,
    w1: Vec<Vec<f32>>,
    b1: Vec<f32>,
    w2: Vec<f32>,
    b2: f32,
}

impl Model {
    pub fn from_json(v: &serde_json::Value) -> Option<Model> {
        let vecf = |x: &serde_json::Value| -> Option<Vec<f32>> {
            x.as_array()?.iter().map(|y| y.as_f64().map(|z| z as f32)).collect()
        };
        Some(Model {
            mean: vecf(&v["mean"])?,
            scale: vecf(&v["scale"])?,
            w1: v["w1"].as_array()?.iter().map(vecf).collect::<Option<Vec<_>>>()?,
            b1: vecf(&v["b1"])?,
            w2: vecf(&v["w2"])?,
            b2: v["b2"].as_f64()? as f32,
        })
    }

    pub fn eval(&self, s: &State) -> f64 {
        let x: Vec<f32> = features(s)
            .iter()
            .zip(self.mean.iter().zip(&self.scale))
            .map(|(v, (m, sc))| (v - m) / sc)
            .collect();
        let mut out = self.b2;
        for (row, (b, w)) in self.w1.iter().zip(self.b1.iter().zip(&self.w2)) {
            let h: f32 = row.iter().zip(&x).map(|(a, b)| a * b).sum::<f32>() + b;
            out += h.max(0.0) * w;
        }
        out as f64
    }
}

/// The trained model, if data/value.json holds one.
pub fn model() -> Option<&'static Model> {
    static M: std::sync::OnceLock<Option<Model>> = std::sync::OnceLock::new();
    M.get_or_init(|| {
        let text = include_str!("../data/value.json");
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
