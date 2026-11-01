//! How the Solver plays the futures after a choice: simple, fast rules of thumb. The Solver's advice is only as good
//! as these futures, so they aim for what a good player does: feed Consume chains, open doors, replace weak Inmates.

use crate::game::*;
use crate::instruments::Instrument;
use crate::rng::Rng;

/// Tuning knobs for testing the rules (env TK="v0,v1,..."; defaults otherwise). Bench only.
fn k(i: usize, default: f64) -> f64 {
    static K: std::sync::OnceLock<Vec<Option<f64>>> = std::sync::OnceLock::new();
    K.get_or_init(|| {
        std::env::var("TK")
            .map(|v| v.split(',').map(|x| x.trim().parse().ok()).collect())
            .unwrap_or_default()
    })
    .get(i)
    .copied()
    .flatten()
    .unwrap_or(default)
}

/// A rough worth of a board in the middle of a run, for comparing treasures and targets: the learned board value when
/// one is loaded and knob 50 is on, otherwise hand-set weights.
pub fn quick_value(s: &State) -> f64 {
    if k(50, 1.0) > 0.0 {
        if let Some(m) = crate::value::model() {
            return m.eval(s) * 100_000.0
                + s.picks as f64 * k(52, 30_000.0)
                + hand_value(s) * k(51, 0.0);
        }
    }
    hand_value(s)
}

fn hand_value(s: &State) -> f64 {
    let w = [1.0, k(14, 4.2), k(15, 18.0), k(13, 42.0), k(12, 70.0)];
    let board: f64 = s
        .inmates()
        .into_iter()
        .map(|i| w[s.inmate(i).unwrap().rarity as usize])
        .sum();
    let eaters = s
        .inmates()
        .into_iter()
        .filter(|&i| !s.targets(i).is_empty())
        .count() as f64;
    let locked = s.locked().len() as f64;
    let open = (CELLS as f64 - 1.0 - locked).max(0.0);
    s.chromatics as f64 * 100_000.0
        + s.picks as f64 * 30_000.0
        + board
        + eaters * k(11, 7.5)
        + open * k(21, 0.5) * (s.days_left as f64).min(k(22, 20.0))
}

fn destination_value(s: &State, i: usize) -> f64 {
    plan_value(s, i) + rule_value(s, i)
}

/// What some instruments make of a destination beyond the cell itself: the walk's length, the doors around it,
/// how full the board is.
fn plan_value(s: &State, i: usize) -> f64 {
    use crate::instruments::Instrument::*;
    let steps = (i + CELLS - s.pos) % CELLS;
    let locked = s.cells[i] == Cell::Locked;
    let mut v = 0.0;
    if s.has(LongChainedShackles) && steps >= 4 {
        v += k(40, 8.0);
    }
    if s.has(ShortChainedShackles) && steps <= 3 {
        v += k(39, 3.0);
    }
    if s.has(LostRingOfKeys) && locked {
        let n = [(i + CELLS - 1) % CELLS, (i + 1) % CELLS]
            .iter()
            .filter(|&&j| s.cells[j] == Cell::Locked)
            .count();
        v += k(41, 2.0) * n as f64;
    }
    // a sure-Chromatic Replace (Noose or Guillotine with the Maiden): a door on an empty board brings an Inmate alone,
    // which the Maiden always takes, so it's replaced into a Chromatic the same day
    if locked && s.has(SeveredIronMaiden) && (s.has(HangmansNoose) || s.has(ScapegoatsGuillotine)) {
        let n = s.inmates().len();
        if n == 0 {
            v += k(45, 0.0);
        } else {
            v += k(46, 0.0) * n as f64;
        }
    }
    // Squirming Iron Box: a nest hatches a Rare Rat if left alone
    if locked && s.nests.iter().any(|n| n.0 == i) {
        v += k(49, -8.0);
    }
    // Raging Beast's Agonizing Muzzle: a Consume pays most with 4 or fewer Inmates on the board
    if s.has(AgonizingMuzzle) && locked && s.inmates().len() >= 4 {
        v += k(38, -6.0);
    }
    v
}

fn rule_value(s: &State, i: usize) -> f64 {
    let incident = if s.has_incident(i) && s.cells[i] != Cell::Empty {
        if matches!(s.objective, Some(Objective::Undercurrents { .. })) {
            20.0
        } else {
            k(7, 20.0)
        }
    } else {
        0.0
    };
    incident
        + match s.cells[i] {
            // a marked Lion or prey: going there can only break the mark
            Cell::Inmate(_) if s.hunt.is_some_and(|h| h.lion == i || h.prey == i) => k(28, -3.0),
            // Blood-Stained Trap: a Lion that can mark a prey
            Cell::Inmate(m)
                if s.has(crate::instruments::Instrument::BloodStainedTrap)
                    && s.hunt.is_none()
                    && m.code == crate::game::Code::Lion
                    && hunt_worth(s, m)
                    && !s.targets(i).is_empty() =>
            {
                k(27, 30.0) + k(2, 1.5) * m.rarity as f64
            }
            // Iron Tongs of Lust: a Tusk-marked Elephant to replace
            Cell::Inmate(m)
                if s.has(crate::instruments::Instrument::IronTongsOfLust)
                    && s.tusks.contains(&i)
                    && m.code == crate::game::Code::Elephant
                    && k(29, 1.0) > 0.5 =>
            {
                k(30, 12.0) + s.inmates().len().min(5) as f64
            }
            // a Replace that brings a Chromatic for sure
            Cell::Inmate(m) if sure_replace(s, i, m) => k(43, 40.0),
            // Severed Iron Maiden: a Replace that comes in 3 tiers up
            Cell::Inmate(m) if s.has(crate::instruments::Instrument::SeveredIronMaiden) && maiden_ready(s, i, m) => {
                k(31, 20.0) + m.rarity as f64
            }
            Cell::Inmate(m) if !s.targets(i).is_empty() => {
                k(1, 3.5)
                    + k(2, 1.5) * m.rarity as f64
                    + if matches!(s.objective, Some(Objective::SurvivalOfTheFittest { .. })) {
                        k(35, 0.0)
                    } else {
                        0.0
                    }
            }
            // an Inmate worth replacing there (more so when an instrument makes the Replace pay)
            Cell::Inmate(m) if m.rarity > RARE && should_replace_at(s, i, m) => {
                4.0 + 2.0 * m.rarity as f64
            }
            Cell::Inmate(m) if m.rarity as f64 <= k(3, 0.0) => k(5, 2.0),
            Cell::Inmate(_) => 0.5,
            Cell::Locked => {
                // a "?" door: its rarity is known (an unknown door holds about a Magic)
                s.revealed_at(i)
                    .map_or(0.0, |r| k(23, 3.0) * (r as f64 - 0.9))
                    + k(0, 8.0)
                    + if s.has(crate::instruments::Instrument::LostRingOfKeys) {
                        1.5
                    } else {
                        0.0
                    }
                    + if matches!(s.objective, Some(Objective::BehindCellDoors { .. })) {
                        k(25, 3.0)
                    } else {
                        0.0
                    }
                    // Executioner's Blacklist: open doors down to 5 locked, then every opening raises 2 Inmates
                    + if s.has(crate::instruments::Instrument::ExecutionersBlacklist) {
                        if s.locked().len() <= 6 {
                            k(37, 40.0)
                        } else {
                            k(36, 6.0)
                        }
                    } else {
                        0.0
                    }
            }
            Cell::Office => k(8, 20.0),
            Cell::Empty => 0.0,
        }
}

pub fn choose_destination(s: &State, options: &[usize], rng: &mut Rng) -> usize {
    // the learned board value: play each destination's day with the rules, judge the board it leaves
    let n = k(47, 0.0) as usize;
    if n > 0 && options.len() > 1 {
        if let Some(m) = crate::value::model() {
            let mut best = options[0];
            let mut best_v = f64::MIN;
            for &o in options {
                let mut v = destination_value(s, o) * k(48, 0.0);
                for _ in 0..n {
                    let mut c = s.clone();
                    c.go(o, rng);
                    resolve_picks(&mut c, rng);
                    decide(&mut c, rng);
                    end_day(&mut c);
                    v += m.eval(&c) / n as f64;
                }
                if v > best_v {
                    best_v = v;
                    best = o;
                }
            }
            return best;
        }
    }
    let n = k(16, 0.0) as usize;
    if n > 0 {
        // look one step ahead: go there and decide, then a rough worth of the board, plus the rule's value
        let mut best = options[0];
        let mut best_v = f64::MIN;
        for &o in options {
            let mut v = destination_value(s, o) * k(17, 0.0);
            for _ in 0..n {
                let mut c = s.clone();
                c.go(o, rng);
                resolve_picks(&mut c, rng);
                decide(&mut c, rng);
                v += quick_value(&c) / n as f64;
            }
            if v > best_v {
                best_v = v;
                best = o;
            }
        }
        return best;
    }
    let mut best = options[0];
    let mut best_v = f64::MIN;
    for &o in options {
        let v = destination_value(s, o) + rng.f64();
        if v > best_v {
            best_v = v;
            best = o;
        }
    }
    best
}

/// The target to eat: the one with the fewest targets of its own, so the Inmates that can eat are kept.
pub fn choose_target(s: &State, targets: &[usize], rng: &mut Rng) -> usize {
    if k(6, 5.0) as i32 == 5 {
        // look one bite ahead: the target leaving the best board (and the eater still hungry)
        let mut best = targets[0];
        let mut best_v = f64::MIN;
        for &t in targets {
            let mut v = 0.0;
            for _ in 0..2 {
                let mut c = s.clone();
                let pos = c.pos;
                c.consume(pos, t, rng);
                v += quick_value(&c) + c.targets(c.pos).len() as f64 * k(24, 20.0);
            }
            if v > best_v {
                best_v = v;
                best = t;
            }
        }
        return best;
    }
    let mut best = targets[0];
    let mut best_v = f64::MAX;
    for &t in targets {
        let r = s.inmate(t).map_or(0.0, |m| m.rarity as f64);
        let v = match k(6, 5.0) as i32 {
            1 => -r,
            3 => -r * 10.0 + s.targets(t).len() as f64,
            4 => -r * 10.0 - s.targets(t).len() as f64,
            2 => r,
            _ => s.targets(t).len() as f64,
        } + rng.f64() * 0.5;
        if v < best_v {
            best_v = v;
            best = t;
        }
    }
    best
}

pub fn choose_treasure(s: &State, rng: &mut Rng) -> Treasure {
    match k(9, 0.0) as i32 {
        1 => return Treasure::CrimsonVintage,
        2 => return Treasure::ThumbRing,
        3 => return Treasure::BrassHorn,
        _ => {}
    }
    let mut best = Treasure::CrimsonVintage;
    let mut best_v = f64::MIN;
    for t in Treasure::ALL {
        let mut v = 0.0;
        for _ in 0..k(10, 4.0) as usize {
            let mut c = s.clone();
            c.treasure(t, rng);
            v += quick_value(&c);
        }
        if v > best_v {
            best_v = v;
            best = t;
        }
    }
    best
}

/// The Torture Instruments from best to worst, as measured by `tartarus rank-instruments` (each held alone from
/// day 1 of a fresh run, 20,000 runs each).
pub const INSTRUMENT_RANK: &[Instrument] = &[
    Instrument::BloodStainedTrap,
    Instrument::IronCrown,
    Instrument::ElephantFootMaul,
    Instrument::KinsCoinOfFate,
    Instrument::FilthyClaws,
    Instrument::SkeweredRatHeads,
    Instrument::ExecutionersBlacklist,
    Instrument::AvengersCasket,
    Instrument::BarbedCollar,
    Instrument::HangmansNoose,
    Instrument::LongChainedShackles,
    Instrument::SortingHook,
    Instrument::RavenousChains,
    Instrument::ScapegoatsGuillotine,
    Instrument::RustySaw,
    Instrument::IronTongsOfLust,
    Instrument::BarbedWhip,
    Instrument::AgonizingMuzzle,
    Instrument::SeveredIronMaiden,
    Instrument::FangedGuillotine,
    Instrument::LostRingOfKeys,
    Instrument::CrowdedBeastcage,
    Instrument::FelonsDogTag,
    Instrument::ShortChainedShackles,
    Instrument::SearingBrandingIron,
    Instrument::SquirmingIronBox,
    Instrument::GaolersExecutionAxe,
    Instrument::WardensScales,
    Instrument::BurningStake,
    Instrument::IronPestle,
    Instrument::ForgedKey,
];

/// Whether to spend the re-roll on an offer: when its best instrument is outside the top half of the ranking.
pub fn should_reroll(offers: &[Instrument]) -> bool {
    let rank = |i: &Instrument| {
        INSTRUMENT_RANK
            .iter()
            .position(|r| r == i)
            .unwrap_or(usize::MAX)
    };
    offers.iter().map(rank).min().unwrap_or(usize::MAX) >= INSTRUMENT_RANK.len() / 2
}

pub fn choose_instrument(offers: &[Instrument]) -> Instrument {
    let rank = |i: &Instrument| {
        INSTRUMENT_RANK
            .iter()
            .position(|r| r == i)
            .unwrap_or(usize::MAX)
    };
    *offers.iter().min_by_key(|i| rank(i)).unwrap()
}

pub fn resolve_picks(s: &mut State, rng: &mut Rng) {
    loop {
        if s.picks > 0 {
            let t = choose_treasure(s, rng);
            s.treasure(t, rng);
        } else if s.offers > 0 {
            s.offers -= 1;
            let mut offer = s.instrument_offer(rng);
            if s.rerolls > 0 && should_reroll(&offer) {
                s.rerolls -= 1;
                offer = s.instrument_offer(rng);
            }
            if !offer.is_empty() {
                s.setup.instruments.add(choose_instrument(&offer));
            }
        } else {
            return;
        }
    }
}

/// Whether to Hunt instead of Consuming (Blood-Stained Trap): with time left for the mark's 3 mornings and a Lion
/// low enough that 3 tiers are worth more than one Consume now.
pub fn should_hunt(s: &State) -> bool {
    s.can_hunt()
        && s.days_left > HUNT_MORNINGS as u32
        && s.inmate(s.pos).is_some_and(|m| hunt_worth(s, m))
        && !s.targets(s.pos).is_empty()
}

/// A Lion worth marking a prey with: low enough that its mornings of growth aren't wasted past Chromatic.
fn hunt_worth(s: &State, m: Inmate) -> bool {
    // with the Iron Crown a Lion's Consume already gains 3 tiers at once
    let most = if s.has(Instrument::IronCrown) {
        k(44, ULTIMATE as f64)
    } else {
        k(26, ULTIMATE as f64)
    };
    m.rarity as f64 <= most
}

/// The prey to mark: the highest rarity (both climb, and a high prey reaches Chromatic too).
pub fn choose_prey(s: &State, targets: &[usize], rng: &mut Rng) -> usize {
    if k(32, 1.0) < 0.5 {
        return choose_target(s, targets, rng);
    }
    *targets
        .iter()
        .max_by_key(|&&t| s.inmate(t).map_or(0, |m| m.rarity))
        .unwrap()
}

/// A Replace here brings a Chromatic for sure: Hangman's Noose (Rare or better) or Scapegoat's Guillotine (above the old
/// rarity) under the Severed Iron Maiden's 3 tiers.
fn sure_replace(s: &State, at: usize, m: Inmate) -> bool {
    use crate::instruments::Instrument::*;
    if k(42, 1.0) < 0.5 || !s.has(SeveredIronMaiden) || !maiden_ready_any(s, at, m) {
        return false;
    }
    let others = s.inmates().len().saturating_sub(1);
    let mut least = if s.has(HangmansNoose) { RARE } else { NORMAL };
    if s.has(ScapegoatsGuillotine) && others >= 2 {
        least = least.max(m.rarity + 1);
    }
    least + 3 >= CHROMATIC
}

/// As many Inmates above as below this one (any rarity).
fn maiden_ready_any(s: &State, at: usize, m: Inmate) -> bool {
    let (mut above, mut below) = (0, 0);
    for j in s.inmates() {
        if j == at {
            continue;
        }
        let r = s.inmate(j).unwrap().rarity;
        if r > m.rarity {
            above += 1;
        } else if r < m.rarity {
            below += 1;
        }
    }
    above == below
}

/// Severed Iron Maiden: as many Inmates above as below this one, so a Replace comes in 3 tiers up.
fn maiden_ready(s: &State, at: usize, m: Inmate) -> bool {
    if m.rarity > LEGENDARY {
        return false;
    }
    let (mut above, mut below) = (0, 0);
    for j in s.inmates() {
        if j == at {
            continue;
        }
        let r = s.inmate(j).unwrap().rarity;
        if r > m.rarity {
            above += 1;
        } else if r < m.rarity {
            below += 1;
        }
    }
    above == below
}

/// Keeps Consuming from the current cell while there are targets. Returns whether anything was eaten.
pub fn consume_chain(s: &mut State, rng: &mut Rng) -> bool {
    let mut ate = false;
    loop {
        if s.hunt.is_some_and(|h| h.lion == s.pos || (k(33, 1.0) > 0.5 && h.prey == s.pos)) {
            return ate;
        }
        let mut t = s.targets(s.pos);
        if k(33, 1.0) > 0.5 {
            if let Some(h) = s.hunt {
                t.retain(|&j| j != h.prey && j != h.lion);
            }
        }
        if t.is_empty() {
            return ate;
        }
        let j = choose_target(s, &t, rng);
        let pos = s.pos;
        s.consume(pos, j, rng);
        resolve_picks(s, rng);
        ate = true;
    }
}

/// Whether to Replace the Inmate in the current cell (when it has nothing to eat), with the instruments that
/// make a Replace pay.
pub fn should_replace(s: &State, m: Inmate) -> bool {
    should_replace_at(s, s.pos, m)
}

fn should_replace_at(s: &State, at: usize, m: Inmate) -> bool {
    use crate::instruments::Instrument::*;
    let others = s
        .inmates()
        .into_iter()
        .filter(|&j| j != at)
        .collect::<Vec<_>>();
    // Scapegoat's Guillotine: the newcomer comes in above the old rarity (an Ultimate becomes a Chromatic), for 2
    // random others
    if s.has(ScapegoatsGuillotine) && others.len() >= 2 {
        // worth 2 random Inmates only for a high one
        return m.rarity >= LEGENDARY;
    }
    // Severed Iron Maiden: as many above as below, the newcomer gains 3 tiers
    if s.has(SeveredIronMaiden) && m.rarity <= LEGENDARY {
        let above = others
            .iter()
            .filter(|&&j| s.inmate(j).unwrap().rarity > m.rarity)
            .count();
        let below = others
            .iter()
            .filter(|&&j| s.inmate(j).unwrap().rarity < m.rarity)
            .count();
        if above == below {
            return true;
        }
    }
    // Iron Tongs of Lust: replacing a Tusk-marked Elephant raises 4 others
    if s.has(IronTongsOfLust)
        && s.tusks.contains(&at)
        && m.code == crate::game::Code::Elephant
        && k(29, 1.0) > 0.5
        && m.rarity <= LEGENDARY
    {
        return true;
    }
    // Hangman's Noose brings Rare or better; the Burning Stake often a Legendary
    if s.has(HangmansNoose) && m.rarity <= RARE {
        return true;
    }
    if s.has(BurningStake) && m.rarity < LEGENDARY {
        return true;
    }
    m.rarity as f64 <= k(3, 0.0) || (m.rarity == RARE && s.days_left as f64 > k(20, 99.0))
}

/// The day's decision in the current cell.
pub fn decide(s: &mut State, rng: &mut Rng) {
    match s.cells[s.pos] {
        Cell::Office => {
            let instigate = match k(18, 2.0) as i32 {
                1 => s.incidents == 0,
                2 => s.days_left as f64 > k(19, 9.8),
                _ => k(4, 1.0) > 0.5,
            };
            if matches!(s.objective, Some(Objective::Undercurrents { .. })) || instigate {
                s.instigate(rng);
            } else {
                s.bestow(rng);
                resolve_picks(s, rng);
            }
        }
        Cell::Inmate(m) => {
            if should_hunt(s) {
                let t: Vec<usize> = s
                    .targets(s.pos)
                    .into_iter()
                    .filter(|&j| s.inmate(j).is_some())
                    .collect();
                if !t.is_empty() {
                    let prey = choose_prey(s, &t, rng);
                    s.start_hunt(prey);
                    return;
                }
            }
            let marked = s.hunt.is_some_and(|h| h.lion == s.pos || h.prey == s.pos);
            if !marked && sure_replace(s, s.pos, m) {
                let pos = s.pos;
                s.replace(pos, rng);
                return;
            }
            // Severed Iron Maiden: a low Inmate ready for 3 tiers is worth more replaced than eating
            if !marked
                && s.has(Instrument::SeveredIronMaiden)
                && (m.rarity as f64) <= k(34, 2.0)
                && maiden_ready(s, s.pos, m)
            {
                let pos = s.pos;
                s.replace(pos, rng);
                return;
            }
            if !consume_chain(s, rng) && !marked && should_replace(s, m) {
                let pos = s.pos;
                s.replace(pos, rng);
            }
        }
        _ => {}
    }
}

pub fn end_day(s: &mut State) {
    s.days_left = s.days_left.saturating_sub(1);
}

/// Plays the run to its end.
pub fn finish(s: &mut State, rng: &mut Rng) {
    while s.days_left > 0 {
        s.morning(rng);
        resolve_picks(s, rng);
        let options = s.destinations(rng);
        let d = choose_destination(s, &options, rng);
        s.go(d, rng);
        resolve_picks(s, rng);
        decide(s, rng);
        end_day(s);
    }
}
