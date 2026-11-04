//! A whole run played by the solver itself (every choice evaluated with `futures` futures a choice), as the guided
//! benchmark plays it and as self-play data is made: `see` is shown the board before each choice. `play_run` plays
//! a run with any chooser (the solver, the policy network, a labeller).

use crate::game::{Cell, Objective, Setup, State};
use crate::rng::Rng;
use crate::solver::{evaluate, evaluate_race, Action, Phase};

/// Run number `k`'s start (its own random draws: the same `k` deals the same run) and its random draws.
pub fn start(k: u64) -> (State, Rng) {
    let mut rng = Rng::new(k.wrapping_mul(0x9E37_79B9) + 7);
    let mut s = State::fresh(Setup::default(), &mut rng);
    s.rerolls = 1;
    s.objective = Some(match k % 5 {
        0 => Objective::PrisonWalk { walked: 0 },
        1 => Objective::KingOfTheCaged,
        2 => Objective::Undercurrents { triggered: 0 },
        3 => Objective::BehindCellDoors { opened: 0 },
        _ => Objective::SurvivalOfTheFittest { consumed: 0 },
    });
    s.offers = 1;
    (s, rng)
}

/// Run number `k`, played by the solver to its end.
pub fn guided_run(k: u64, futures: usize, race: bool, mut see: impl FnMut(&State)) -> State {
    let (s, mut rng) = start(k);
    play_run(s, &mut rng, &mut |s: &State, p: &Phase, rng: &mut Rng| {
        see(s);
        let sd = (rng.f64() * 1e9) as u64;
        let out = if race { evaluate_race(s, p, futures, sd) } else { evaluate(s, p, futures, sd) };
        out[0].action
    })
}

/// Plays `s` to the run's end, `choose` making every choice (Sewer Ratway jumps, treasure picks, instrument offers,
/// the day's destination and the decision in the cell).
pub fn play_run(
    mut s: State,
    rng: &mut Rng,
    choose: &mut dyn FnMut(&State, &Phase, &mut Rng) -> Action,
) -> State {
    picks(&mut s, rng, choose);
    while s.days_left > 0 {
        s.morning(rng);
        picks(&mut s, rng, choose);
        let options = s.destinations(rng);
        if let Action::Go(d) = choose(&s, &Phase::Move(options), rng) {
            s.go(d, rng);
        }
        picks(&mut s, rng, choose);
        decide_day(&mut s, rng, choose);
        s.days_left -= 1;
    }
    s
}

/// The decisions in the current cell (Consumes go on while there is something to eat).
pub fn decide_day(s: &mut State, rng: &mut Rng, choose: &mut dyn FnMut(&State, &Phase, &mut Rng) -> Action) {
    loop {
        if !matches!(s.cells[s.pos], Cell::Office | Cell::Inmate(_)) {
            break;
        }
        let a = choose(s, &Phase::Decide, rng);
        let pos = s.pos;
        match a {
            Action::Consume(j) => {
                s.consume(pos, j, rng);
                picks(s, rng, choose);
                continue;
            }
            Action::Hunt(j) => s.start_hunt(j),
            Action::Replace => s.replace(pos, rng),
            Action::Bestow => s.bestow(rng),
            Action::Instigate => s.instigate(rng),
            _ => {}
        }
        picks(s, rng, choose);
        break;
    }
}

/// Sewer Ratway jumps, treasure picks and instrument offers as they come.
pub fn picks(s: &mut State, rng: &mut Rng, choose: &mut dyn FnMut(&State, &Phase, &mut Rng) -> Action) {
    loop {
        if s.ratway {
            let options = s.warp_options();
            match choose(s, &Phase::Move(options), rng) {
                Action::Go(d) => s.warp(d, rng),
                _ => s.ratway = false,
            }
        } else if s.picks > 0 {
            if let Action::Pick(t) = choose(s, &Phase::Treasure, rng) {
                s.treasure(t, rng);
            } else {
                s.picks -= 1;
            }
        } else if s.offers > 0 {
            s.offers -= 1;
            let mut offer = s.instrument_offer(rng);
            loop {
                match choose(s, &Phase::Instrument(offer.clone()), rng) {
                    Action::Take(i) => {
                        s.setup.instruments.add(i);
                        break;
                    }
                    Action::Reroll if s.rerolls > 0 => {
                        s.rerolls -= 1;
                        offer = s.instrument_offer(rng);
                    }
                    _ => break,
                }
            }
        } else {
            break;
        }
    }
}
