//! A learned chooser (experimental): a small network that scores each option of a choice from the board and the
//! board one step after taking it, trained to agree with the solver's own evaluations (examples/policy_data.rs,
//! tools/train_policy.py). It plays a choice in microseconds, where the solver plays hundreds of futures.

use crate::game::*;
use crate::instruments::ALL;
use crate::rng::Rng;
use crate::solver::{actions, Action, Phase};
use crate::value::{features, N_FEATURES};

/// The option's kinds, in input order.
const KINDS: usize = 10;
pub const N_INPUT: usize = KINDS + 2 * N_FEATURES + 4 + ALL.len() + 8;

fn kind(a: &Action) -> usize {
    match a {
        Action::Go(_) => 0,
        Action::Consume(_) => 1,
        Action::Hunt(_) => 2,
        Action::Replace => 3,
        Action::End => 4,
        Action::Bestow => 5,
        Action::Instigate => 6,
        Action::Pick(_) => 7,
        Action::Take(_) => 8,
        Action::Reroll => 9,
    }
}

/// The board one step after `a` (its own random draws from `rng`): what the option does right away.
pub fn after(s: &State, a: Action, rng: &mut Rng) -> State {
    let mut c = s.clone();
    let pos = c.pos;
    match a {
        Action::Go(i) => {
            if c.ratway {
                c.warp(i, rng)
            } else {
                c.go(i, rng)
            }
        }
        Action::Consume(j) => c.consume(pos, j, rng),
        Action::Hunt(j) => c.start_hunt(j),
        Action::Replace => c.replace(pos, rng),
        Action::End | Action::Reroll => {}
        Action::Bestow => c.bestow(rng),
        Action::Instigate => c.instigate(rng),
        Action::Pick(t) => c.treasure(t, rng),
        Action::Take(i) => c.setup.instruments.add(i),
    }
    c
}

/// The network's input for option `a` on board `s` (`base`: the board's features, the same for every option).
pub fn input(s: &State, base: &[f32], a: Action, rng: &mut Rng, out: &mut Vec<f32>) {
    out.clear();
    for k in 0..KINDS {
        out.push((kind(&a) == k) as u8 as f32);
    }
    out.extend_from_slice(base);
    let next = features(&after(s, a, rng));
    out.extend(next.iter().zip(base).map(|(n, b)| n - b));
    for t in Treasure::ALL {
        out.push((a == Action::Pick(t)) as u8 as f32);
    }
    for i in ALL {
        out.push((a == Action::Take(i)) as u8 as f32);
    }
    // the cell the option is about (where to go, what to eat or hunt)
    let cell = match a {
        Action::Go(i) | Action::Consume(i) | Action::Hunt(i) => Some(i),
        _ => None,
    };
    let c = cell.map(|i| s.cells[i]);
    out.push(matches!(c, Some(Cell::Locked)) as u8 as f32);
    out.push(matches!(c, Some(Cell::Office)) as u8 as f32);
    out.push(matches!(c, Some(Cell::Empty)) as u8 as f32);
    let m = cell.and_then(|i| s.inmate(i));
    out.push(m.map_or(0.0, |m| m.rarity as f32 + 1.0));
    for code in Code::ALL {
        out.push(m.is_some_and(|m| m.code == code) as u8 as f32);
    }
    debug_assert_eq!(out.len(), N_INPUT);
}

/// A network of fully connected layers (ReLU between them) from N_INPUT numbers to one score. The first layer has
/// the inputs' normalisation folded in and is kept by input, so that zero inputs are skipped, and the board's own
/// part (the same for every option of a choice) is added once per choice.
pub struct Net {
    /// First layer, by input: hidden k of input i at w1[i * h + k].
    w1: Vec<f32>,
    b1: Vec<f32>,
    h: usize,
    /// The other layers' weights (out by in, row by row), biases and input widths.
    rest: Vec<(Vec<f32>, Vec<f32>, usize)>,
}

const BASE: std::ops::Range<usize> = KINDS..KINDS + N_FEATURES;
const MAX_H: usize = 256;

impl Net {
    pub fn from_json(v: &serde_json::Value) -> Option<Net> {
        let vecf = |x: &serde_json::Value| -> Option<Vec<f32>> {
            x.as_array()?.iter().map(|y| y.as_f64().map(|z| z as f32)).collect()
        };
        let mean = vecf(&v["mean"])?;
        let scale = vecf(&v["scale"])?;
        if mean.len() != N_INPUT || scale.len() != N_INPUT {
            return None;
        }
        let layers = v["layers"].as_array()?;
        let (w, b) = (vecf(&layers.first()?["w"])?, vecf(&layers.first()?["b"])?);
        let h = b.len();
        if w.len() != h * N_INPUT || h > MAX_H {
            return None;
        }
        let mut w1 = vec![0f32; h * N_INPUT];
        let mut b1 = b.clone();
        for k in 0..h {
            for i in 0..N_INPUT {
                let x = w[k * N_INPUT + i] / scale[i];
                w1[i * h + k] = x;
                b1[k] -= x * mean[i];
            }
        }
        let mut rest = Vec::new();
        let mut width = h;
        for l in &layers[1..] {
            let (w, b) = (vecf(&l["w"])?, vecf(&l["b"])?);
            if w.len() != b.len() * width || b.len() > MAX_H {
                return None;
            }
            rest.push((w, b.clone(), width));
            width = b.len();
        }
        (width == 1).then_some(Net { w1, b1, h, rest })
    }

    #[inline]
    fn add(&self, acc: &mut [f32], i: usize, x: f32) {
        if x != 0.0 {
            let col = &self.w1[i * self.h..(i + 1) * self.h];
            for (a, w) in acc.iter_mut().zip(col) {
                *a += x * w;
            }
        }
    }

    /// The first layer's sums over the board's own part, plus the bias.
    fn base(&self, base: &[f32]) -> [f32; MAX_H] {
        let mut acc = [0f32; MAX_H];
        acc[..self.h].copy_from_slice(&self.b1);
        for (n, &x) in base.iter().enumerate() {
            self.add(&mut acc[..self.h], BASE.start + n, x);
        }
        acc
    }

    /// The score of an option from its input `x`, the board's part already in `acc`.
    fn finish(&self, mut acc: [f32; MAX_H], x: &[f32]) -> f32 {
        let h = self.h;
        for (i, &v) in x.iter().enumerate() {
            if !BASE.contains(&i) {
                self.add(&mut acc[..h], i, v);
            }
        }
        let mut cur = [0f32; MAX_H];
        for (c, a) in cur.iter_mut().zip(&acc[..h]) {
            *c = a.max(0.0);
        }
        let mut width = h;
        let last = self.rest.len() - 1;
        for (n, (w, b, _)) in self.rest.iter().enumerate() {
            let mut next = [0f32; MAX_H];
            for (k, o) in next[..b.len()].iter_mut().enumerate() {
                let row = &w[k * width..(k + 1) * width];
                let mut z = b[k];
                for (a, c) in row.iter().zip(&cur[..width]) {
                    z += a * c;
                }
                *o = if n < last { z.max(0.0) } else { z };
            }
            cur = next;
            width = b.len();
        }
        cur[0]
    }

    pub fn score(&self, x: &[f32]) -> f32 {
        self.finish(self.base(&x[BASE]), x)
    }

    /// Every option's score (higher is better; a relative value, not Chromatics).
    pub fn scores(&self, s: &State, phase: &Phase, rng: &mut Rng) -> Vec<(Action, f32)> {
        let acts = actions(s, phase);
        let base = features(s);
        let acc = self.base(&base);
        let mut x = Vec::with_capacity(N_INPUT);
        acts.into_iter()
            .map(|a| {
                input(s, &base, a, rng, &mut x);
                (a, self.finish(acc, &x))
            })
            .collect()
    }

    /// The option the network rates best.
    pub fn choose(&self, s: &State, phase: &Phase, rng: &mut Rng) -> Action {
        let acts = actions(s, phase);
        if acts.len() <= 1 {
            return acts.first().copied().unwrap_or(Action::End);
        }
        let base = features(s);
        let acc = self.base(&base);
        let mut x = Vec::with_capacity(N_INPUT);
        let mut best = (f32::MIN, acts[0]);
        for a in acts {
            input(s, &base, a, rng, &mut x);
            let v = self.finish(acc, &x);
            if v > best.0 {
                best = (v, a);
            }
        }
        best.1
    }
}

/// The network built in (data/policy.json), in the builds with the network.
#[cfg(any(feature = "net", feature = "net-only"))]
const BUILT_IN: Option<&str> = Some(include_str!("../data/policy.json"));
#[cfg(not(any(feature = "net", feature = "net-only")))]
const BUILT_IN: Option<&str> = None;

/// The network in env POLICY_JSON, else the one built in, if any.
pub fn net() -> Option<&'static Net> {
    static N: std::sync::OnceLock<Option<Net>> = std::sync::OnceLock::new();
    N.get_or_init(|| {
        let text = match std::env::var("POLICY_JSON") {
            Ok(p) => std::fs::read_to_string(p).ok()?,
            Err(_) => BUILT_IN?.to_string(),
        };
        Net::from_json(&serde_json::from_str(&text).ok()?)
    })
    .as_ref()
}

/// The network alone picks (no futures): the net-only build, or env NET_ONLY with a network.
pub fn net_only() -> Option<&'static Net> {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| cfg!(feature = "net-only") || std::env::var("NET_ONLY").is_ok()) {
        net()
    } else {
        None
    }
}

/// Outcomes from the network's scores alone, best first: its score stands in for the Chromatics (and Ultimates), with
/// no margin and no share at 10.
pub fn outcomes(net: &Net, s: &State, phase: &Phase, seed: u64) -> Vec<crate::solver::Outcome> {
    let mut rng = Rng::new(seed);
    let mut out: Vec<crate::solver::Outcome> = net
        .scores(s, phase, &mut rng)
        .into_iter()
        .map(|(action, v)| crate::solver::Outcome {
            action,
            score: v as f64,
            chromatics: v as f64,
            ultimates: v as f64,
            full: 0.0,
            margin: 0.0,
            ult_margin: 0.0,
        })
        .collect();
    out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
    out
}
