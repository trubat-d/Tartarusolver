//! The Solver: for each option you have now, it plays many random futures to the end of the run and compares how
//! they end (see `State::score`). Every option is played on the same futures, so the differences are the options'.

use crate::game::*;
use crate::instruments::Instrument;
use crate::policy;
use crate::rng::Rng;
use rayon::prelude::*;

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
    policy::finish(&mut s, rng);
    s
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
}

/// The average over `futures` futures of the run that `start` plays (each future its own random draws, the same
/// draws for every option compared: common random numbers).
fn average(
    futures: usize,
    seed: u64,
    start: impl Fn(&mut Rng) -> State + Sync,
) -> (f64, f64, f64, f64, f64) {
    let (score, chroma, ult, full, sq) = (0..futures)
        .into_par_iter()
        .map(|k| {
            let mut rng = Rng::new(seed.wrapping_add(k as u64 * 0x2545_F491_4F6C_DD1D));
            let end = start(&mut rng);
            let c = end.chromatics as f64;
            (
                end.score(),
                c,
                end.ultimates() as f64,
                (end.chromatics >= MAX_CHROMATICS) as u8 as f64,
                c * c,
            )
        })
        .reduce(
            || (0.0, 0.0, 0.0, 0.0, 0.0),
            |x, y| (x.0 + y.0, x.1 + y.1, x.2 + y.2, x.3 + y.3, x.4 + y.4),
        );
    let n = futures as f64;
    let mean = chroma / n;
    let var = (sq / n - mean * mean).max(0.0);
    (score / n, mean, ult / n, full / n, 2.0 * (var / n).sqrt())
}

fn evaluate_one(s: &State, a: Action, futures: usize, seed: u64) -> Outcome {
    let (score, chromatics, ultimates, full, margin) =
        average(futures, seed, |rng| play(s.clone(), a, rng));
    Outcome {
        action: a,
        score,
        chromatics,
        ultimates,
        full,
        margin,
    }
}

/// Like `evaluate`, then the options within the margin of the best are played again with 4 times the futures, so a
/// close call gets a sharper answer while clear ones stay fast.
pub fn evaluate_adaptive(s: &State, phase: &Phase, futures: usize, seed: u64) -> Vec<Outcome> {
    let mut out = evaluate(s, phase, futures, seed);
    if out.len() < 2 {
        return out;
    }
    let best = out[0].chromatics;
    let close: Vec<usize> = (0..out.len())
        .filter(|&k| best - out[k].chromatics <= out[0].margin.max(out[k].margin))
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
    let mut out: Vec<(Outcome, Treasure)> = Treasure::ALL
        .into_iter()
        .map(|t1| {
            Treasure::ALL
                .into_iter()
                .map(|t2| {
                    let (score, chromatics, ultimates, full, margin) =
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
    let mut out: Vec<Outcome> = actions(s, phase)
        .into_iter()
        .map(|a| evaluate_one(s, a, futures, seed))
        .collect();
    out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
    out
}
