//! The Solver: for each option you have now, it plays many random futures to the end of the run and compares how
//! they end (see `State::score`). Every option is played on the same futures, so the differences are the options'.

use crate::game::*;
use crate::instruments::Instrument;
use crate::policy;
use crate::rng::Rng;
use rayon::prelude::*;

/// How far the solver is in its current evaluation: futures played and futures planned (for a progress bar).
pub static PROGRESS_DONE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
pub static PROGRESS_TOTAL: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn progress_start(total: usize) {
    use std::sync::atomic::Ordering::Relaxed;
    PROGRESS_DONE.store(0, Relaxed);
    PROGRESS_TOTAL.store(total.max(1), Relaxed);
}

fn progress_add(n: usize) {
    PROGRESS_DONE.fetch_add(n, std::sync::atomic::Ordering::Relaxed);
}

/// The share of the current evaluation done, 0 to 1.
pub fn progress() -> f64 {
    use std::sync::atomic::Ordering::Relaxed;
    (PROGRESS_DONE.load(Relaxed) as f64 / PROGRESS_TOTAL.load(Relaxed).max(1) as f64).min(1.0)
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Action {
    Go(usize),
    Consume(usize),
    Hunt(usize),
    Replace,
    End,
    Bestow,
    Instigate,
    Pick(Treasure),
    Take(Instrument),
    /// Re-roll the Torture Instrument offer.
    Reroll,
}

impl Action {
    /// A short stable name: go:9, consume:12, hunt:5, replace, end, bestow, instigate, treasure:<name>,
    /// instrument:<name> (cells numbered from 1).
    pub fn key(&self) -> String {
        match *self {
            Action::Go(i) => format!("go:{}", i + 1),
            Action::Consume(i) => format!("consume:{}", i + 1),
            Action::Hunt(i) => format!("hunt:{}", i + 1),
            Action::Replace => "replace".into(),
            Action::End => "end".into(),
            Action::Bestow => "bestow".into(),
            Action::Instigate => "instigate".into(),
            Action::Pick(t) => format!("treasure:{}", t.name()),
            Action::Take(i) => format!("instrument:{}", i.name()),
            Action::Reroll => "reroll".into(),
        }
    }

    pub fn describe(&self, s: &State) -> String {
        let cell = |i: usize| match s.inmate(i) {
            Some(m) => format!(
                "cell {} ({} {})",
                i + 1,
                RARITY_NAMES[m.rarity as usize],
                m.code.name()
            ),
            None if i == OFFICE => "the Interrogation Room".to_string(),
            None if s.cells[i] == Cell::Locked => format!("cell {} (locked)", i + 1),
            None => format!("cell {}", i + 1),
        };
        match *self {
            Action::Go(i) => format!("Go to {}", cell(i)),
            Action::Consume(i) if s.cells[i] == Cell::Locked => {
                format!("Consume behind scratched cell {}", i + 1)
            }
            Action::Consume(i) => format!("Consume {}", cell(i)),
            Action::Hunt(i) => format!("Hunt {}", cell(i)),
            Action::Replace => "Replace".into(),
            Action::End if s.bites_today > 0 => "Stop eating (Cancel, then End)".into(),
            Action::End => "End".into(),
            Action::Bestow => "Bestow".into(),
            Action::Instigate => "Instigate".into(),
            Action::Pick(t) => format!("Take {}", t.name()),
            Action::Take(i) => format!("Take {}", i.name()),
            Action::Reroll => "Re-roll the offer".into(),
        }
    }
}

/// What you're choosing now.
#[derive(Clone, Debug)]
pub enum Phase {
    /// Picking among these destinations.
    Move(Vec<usize>),
    /// Deciding in the current cell.
    Decide,
    /// Picking a Warden's Treasure.
    Treasure,
    /// Picking one of the Torture Instruments offered (the re-roll, when left, is `State::rerolls`); then the day
    /// goes on.
    Instrument(Vec<Instrument>),
}

pub fn actions(s: &State, phase: &Phase) -> Vec<Action> {
    match phase {
        Phase::Move(options) => options.iter().map(|&o| Action::Go(o)).collect(),
        Phase::Treasure => Treasure::ALL.into_iter().map(Action::Pick).collect(),
        Phase::Instrument(offers) => {
            let mut a: Vec<Action> = offers.iter().map(|&i| Action::Take(i)).collect();
            if s.rerolls > 0 {
                a.push(Action::Reroll);
            }
            a
        }
        Phase::Decide => match s.cells[s.pos] {
            Cell::Office => vec![Action::Bestow, Action::Instigate],
            Cell::Inmate(_) => {
                let mut a: Vec<Action> =
                    s.targets(s.pos).into_iter().map(Action::Consume).collect();
                if s.can_hunt() {
                    a.extend(
                        s.targets(s.pos)
                            .into_iter()
                            .filter(|&j| s.inmate(j).is_some())
                            .map(Action::Hunt),
                    );
                }
                // a day has a Consume or a Replace, not both: no Replace once something was eaten today
                if s.bites_today == 0 {
                    a.push(Action::Replace);
                }
                a.push(Action::End);
                a
            }
            _ => vec![Action::End],
        },
    }
}

/// Plays `a` now, then the rest of the run.
fn play(mut s: State, a: Action, rng: &mut Rng) -> State {
    match a {
        Action::Go(i) => {
            s.go(i, rng);
            policy::resolve_picks(&mut s, rng);
            policy::decide(&mut s, rng);
        }
        Action::Consume(j) => {
            let pos = s.pos;
            s.consume(pos, j, rng);
            policy::resolve_picks(&mut s, rng);
            policy::consume_chain(&mut s, rng);
        }
        Action::Hunt(j) => s.start_hunt(j),
        Action::Replace => {
            let pos = s.pos;
            s.replace(pos, rng);
        }
        Action::End => {}
        Action::Instigate => s.instigate(rng),
        Action::Bestow => {
            s.bestow(rng);
            policy::resolve_picks(&mut s, rng);
        }
        Action::Take(i) => {
            s.setup.instruments.add(i);
            policy::decide(&mut s, rng);
        }
        Action::Reroll => {
            // a new offer of 3, taken by the measured ranking; the re-roll is gone for the objective's offer
            s.rerolls = s.rerolls.saturating_sub(1);
            let offer = s.instrument_offer(rng);
            if !offer.is_empty() {
                s.setup.instruments.add(policy::choose_instrument(&offer));
            }
            policy::decide(&mut s, rng);
        }
        Action::Pick(t) => {
            s.treasure(t, rng);
            policy::resolve_picks(&mut s, rng);
            policy::consume_chain(&mut s, rng);
        }
    }
    policy::end_day(&mut s);
    match horizon() {
        Some(h) => policy::finish_for(&mut s, rng, h),
        None => policy::finish(&mut s, rng),
    }
    s
}

/// Futures cut short after this many days and finished by the board value (env TH), when a model is loaded.
fn horizon() -> Option<u32> {
    static H: std::sync::OnceLock<Option<u32>> = std::sync::OnceLock::new();
    *H.get_or_init(|| {
        std::env::var("TH")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|&h: &u32| h > 0 && crate::value::has_model())
    })
}

/// A future's end as the solver counts it: the run's result, or, for a future cut short, the board value's estimate.
fn result(end: &State) -> (f64, f64, f64, f64) {
    if end.days_left > 0 {
        if let Some(m) = crate::value::model() {
            let c = m.eval(end).clamp(end.chromatics as f64, MAX_CHROMATICS as f64);
            let full = crate::value::model10().map_or(0.0, |m10| m10.eval(end).clamp(0.0, 1.0));
            let ult = end.ultimates() as f64;
            let score = c * 100_000.0 + full * 300_000.0 + ult * 10.0;
            return (score, c, ult, full);
        }
    }
    (
        end.score(),
        end.chromatics as f64,
        end.ultimates() as f64,
        (end.chromatics >= MAX_CHROMATICS) as u8 as f64,
    )
}

#[derive(Clone, Debug)]
pub struct Outcome {
    pub action: Action,
    pub score: f64,
    /// Average Chromatics at the end of the run.
    pub chromatics: f64,
    /// Average Ultimates left on the board.
    pub ultimates: f64,
    /// Share of futures that reach the 10 Chromatics.
    pub full: f64,
    /// The margin on `chromatics` (two standard errors): options closer than this are a toss-up.
    pub margin: f64,
    /// The margin on `ultimates`.
    pub ult_margin: f64,
}

/// The average over `futures` futures of the run that `start` plays (each future its own random draws, the same
/// draws for every option compared: common random numbers).
fn average(
    futures: usize,
    seed: u64,
    start: impl Fn(&mut Rng) -> State + Sync,
) -> (f64, f64, f64, f64, f64, f64) {
    let (score, chroma, ult, full, sq, usq) = (0..futures)
        .into_par_iter()
        .map(|k| {
            let mut rng = Rng::new(seed.wrapping_add(k as u64 * 0x2545_F491_4F6C_DD1D));
            let end = start(&mut rng);
            progress_add(1);
            let (score, c, ult, full) = result(&end);
            (score, c, ult, full, c * c, ult * ult)
        })
        .reduce(
            || (0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
            |x, y| (x.0 + y.0, x.1 + y.1, x.2 + y.2, x.3 + y.3, x.4 + y.4, x.5 + y.5),
        );
    let n = futures as f64;
    let mean = chroma / n;
    let var = (sq / n - mean * mean).max(0.0);
    let um = ult / n;
    let uvar = (usq / n - um * um).max(0.0);
    (score / n, mean, um, full / n, 2.0 * (var / n).sqrt(), 2.0 * (uvar / n).sqrt())
}

/// Running sums over the futures played so far for one option (futures numbered from 0, the same numbers for every
/// option: common random numbers).
#[derive(Clone, Copy, Default)]
struct Sums {
    n: f64,
    score: f64,
    chroma: f64,
    ult: f64,
    full: f64,
    sq: f64,
    usq: f64,
}

impl Sums {
    /// Plays futures `from..to` of option `a` and adds them.
    fn add(&mut self, s: &State, a: Action, from: usize, to: usize, seed: u64) {
        let (score, chroma, ult, full, sq, usq) = (from..to)
            .into_par_iter()
            .map(|k| {
                let mut rng = Rng::new(seed.wrapping_add(k as u64 * 0x2545_F491_4F6C_DD1D));
                let end = play(s.clone(), a, &mut rng);
                progress_add(1);
                let (score, c, ult, full) = result(&end);
                (score, c, ult, full, c * c, ult * ult)
            })
            .reduce(
                || (0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
                |x, y| (x.0 + y.0, x.1 + y.1, x.2 + y.2, x.3 + y.3, x.4 + y.4, x.5 + y.5),
            );
        self.n += (to - from) as f64;
        self.score += score;
        self.chroma += chroma;
        self.ult += ult;
        self.full += full;
        self.sq += sq;
        self.usq += usq;
    }

    fn outcome(&self, action: Action) -> Outcome {
        let n = self.n.max(1.0);
        let mean = self.chroma / n;
        let var = (self.sq / n - mean * mean).max(0.0);
        let um = self.ult / n;
        let uvar = (self.usq / n - um * um).max(0.0);
        Outcome {
            action,
            score: self.score / n,
            chromatics: mean,
            ultimates: um,
            full: self.full / n,
            margin: 2.0 * (var / n).sqrt(),
            ult_margin: 2.0 * (uvar / n).sqrt(),
        }
    }
}

/// Like `evaluate_adaptive`, as a race: every option is played on a quarter of the futures first, the ones clearly
/// behind the best (by more than both margins together) stop there, the others go on, doubling, up to `futures`;
/// then the close calls get 4 times the futures. Most of the work goes to the options that could be best.
pub fn evaluate_race(s: &State, phase: &Phase, futures: usize, seed: u64) -> Vec<Outcome> {
    let acts = actions(s, phase);
    if acts.len() < 2 {
        return evaluate(s, phase, futures, seed);
    }
    let at10 = s.chromatics >= MAX_CHROMATICS;
    let val = |o: &Outcome| if at10 { (o.ultimates, o.ult_margin) } else { (o.chromatics, o.margin) };
    // planned: every option to `futures`, plus the close calls (a guess: two options to 4 times); dropping options
    // early makes the bar jump ahead
    progress_start(acts.len() * futures + 2 * 3 * futures);
    let mut sums = vec![Sums::default(); acts.len()];
    let mut alive: Vec<usize> = (0..acts.len()).collect();
    let mut done = 0;
    let mut n = (futures / 4).max(50).min(futures);
    loop {
        for &k in &alive {
            sums[k].add(s, acts[k], done, n, seed);
        }
        done = n;
        if done >= futures {
            break;
        }
        let outs: Vec<(usize, (f64, f64))> = alive.iter().map(|&k| (k, val(&sums[k].outcome(acts[k])))).collect();
        let best = outs.iter().map(|o| o.1).fold((f64::MIN, 0.0), |b, x| if x.0 > b.0 { x } else { b });
        let before = alive.len();
        alive.retain(|&k| {
            let v = outs.iter().find(|o| o.0 == k).unwrap().1;
            best.0 - v.0 <= best.1 + v.1
        });
        // the dropped options' remaining futures won't be played
        progress_add((before - alive.len()) * (futures - done));
        if alive.len() < 2 {
            break;
        }
        n = (n * 2).min(futures);
    }
    // the close calls: up to 4 options within the margin of the best, played to 4 times the futures
    let mut outs: Vec<(usize, Outcome)> = alive.iter().map(|&k| (k, sums[k].outcome(acts[k]))).collect();
    outs.sort_by(|a, b| b.1.score.partial_cmp(&a.1.score).unwrap());
    if outs.len() >= 2 {
        let b = val(&outs[0].1);
        let close: Vec<usize> = outs
            .iter()
            .filter(|o| b.0 - val(&o.1).0 <= b.1.max(val(&o.1).1))
            .take(4)
            .map(|o| o.0)
            .collect();
        if close.len() >= 2 {
            let to = futures * 4;
            for k in close {
                let from = sums[k].n as usize;
                if from < to {
                    sums[k].add(s, acts[k], from, to, seed);
                }
            }
        }
    }
    let mut out: Vec<Outcome> = (0..acts.len()).map(|k| sums[k].outcome(acts[k])).collect();
    out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
    out
}

fn evaluate_one(s: &State, a: Action, futures: usize, seed: u64) -> Outcome {
    let (score, chromatics, ultimates, full, margin, ult_margin) =
        average(futures, seed, |rng| play(s.clone(), a, rng));
    Outcome {
        action: a,
        score,
        chromatics,
        ultimates,
        full,
        margin,
        ult_margin,
    }
}

/// Like `evaluate`, then the options within the margin of the best are played again with 4 times the futures, so a
/// close call gets a sharper answer while clear ones stay fast.
pub fn evaluate_adaptive(s: &State, phase: &Phase, futures: usize, seed: u64) -> Vec<Outcome> {
    let mut out = evaluate(s, phase, futures, seed);
    if out.len() < 2 {
        return out;
    }
    let at10 = s.chromatics >= MAX_CHROMATICS;
    let val = |o: &Outcome| if at10 { (o.ultimates, o.ult_margin) } else { (o.chromatics, o.margin) };
    let best = val(&out[0]).0;
    let close: Vec<usize> = (0..out.len())
        .filter(|&k| best - val(&out[k]).0 <= val(&out[0]).1.max(val(&out[k]).1))
        .take(4)
        .collect();
    if close.len() < 2 {
        return out;
    }
    for k in close {
        out[k] = evaluate_one(s, out[k].action, futures * 4, seed);
    }
    out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
    out
}

/// With 2 or more Warden's Treasure picks to make, each first pick is judged with the best second pick after it (the
/// rest by the rules), since the order matters (opening cells first, then raising them). Returns each first pick
/// with its best follow-up.
pub fn evaluate_treasure_pairs(s: &State, futures: usize, seed: u64) -> Vec<(Outcome, Treasure)> {
    progress_start(Treasure::ALL.len() * Treasure::ALL.len() * futures);
    let mut out: Vec<(Outcome, Treasure)> = Treasure::ALL
        .into_iter()
        .map(|t1| {
            Treasure::ALL
                .into_iter()
                .map(|t2| {
                    let (score, chromatics, ultimates, full, margin, ult_margin) =
                        average(futures, seed, |rng| {
                            let mut c = s.clone();
                            c.treasure(t1, rng);
                            if c.picks > 0 {
                                c.treasure(t2, rng);
                            }
                            policy::resolve_picks(&mut c, rng);
                            policy::consume_chain(&mut c, rng);
                            policy::end_day(&mut c);
                            policy::finish(&mut c, rng);
                            c
                        });
                    (
                        Outcome {
                            action: Action::Pick(t1),
                            score,
                            chromatics,
                            ultimates,
                            full,
                            margin,
                            ult_margin,
                        },
                        t2,
                    )
                })
                .max_by(|a, b| a.0.score.partial_cmp(&b.0.score).unwrap())
                .unwrap()
        })
        .collect();
    out.sort_by(|a, b| b.0.score.partial_cmp(&a.0.score).unwrap());
    out
}

pub fn evaluate(s: &State, phase: &Phase, futures: usize, seed: u64) -> Vec<Outcome> {
    progress_start(actions(s, phase).len() * futures);
    let mut out: Vec<Outcome> = actions(s, phase)
        .into_iter()
        .map(|a| evaluate_one(s, a, futures, seed))
        .collect();
    out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
    out
}
