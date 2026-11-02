//! The rules of Tartarus Prison (see RULES.md): the board, opening cells, Consume, Replace, the Interrogation Room,
//! Chromatics, the Warden's Treasures and the Torture Instruments. Odds not known from the game are the community
//! simulator's.

use crate::instruments::{Held, Instrument::*};
use crate::rng::Rng;

pub const CELLS: usize = 16;
/// The Interrogation Room: cell 1, index 0.
pub const OFFICE: usize = 0;
pub const MAX_CHROMATICS: u8 = 10;

pub const NORMAL: u8 = 0;
pub const MAGIC: u8 = 1;
pub const RARE: u8 = 2;
pub const LEGENDARY: u8 = 3;
pub const ULTIMATE: u8 = 4;
pub const CHROMATIC: u8 = 5;
pub const RARITY_NAMES: [&str; 6] = [
    "Normal",
    "Magic",
    "Rare",
    "Legendary",
    "Ultimate",
    "Chromatic",
];

/// Rarity of a new Inmate behind a locked door, Normal to Ultimate (community simulator, observed boards).
const DOOR_ODDS: [f64; 5] = [0.40, 0.38, 0.12, 0.08, 0.02];
/// Rarity of a new Inmate from a Replace (players' boards, 261 Replaces, community simulator; the user's plain
/// Replaces agree: 14 of 14 Normal or Magic).
const REPLACE_ODDS: [f64; 5] = [0.517, 0.314, 0.092, 0.061, 0.015];
/// How many mornings a Prey mark lasts.
pub const HUNT_MORNINGS: u8 = 3;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Code {
    Elephant,
    Lion,
    Wolf,
    Rat,
}

impl Code {
    pub const ALL: [Code; 4] = [Code::Elephant, Code::Lion, Code::Wolf, Code::Rat];

    /// Whether this Code counters `prey` (the default food chain).
    pub fn counters(self, prey: Code) -> bool {
        use Code::*;
        matches!(
            (self, prey),
            (Elephant, Lion)
                | (Elephant, Wolf)
                | (Lion, Wolf)
                | (Lion, Rat)
                | (Wolf, Rat)
                | (Rat, Elephant)
        )
    }

    pub fn name(self) -> &'static str {
        match self {
            Code::Elephant => "Elephant",
            Code::Lion => "Lion",
            Code::Wolf => "Wolf",
            Code::Rat => "Rat",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Inmate {
    pub code: Code,
    pub rarity: u8,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cell {
    Locked,
    /// Opened, with no Inmate in it.
    Empty,
    Office,
    Inmate(Inmate),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Treasure {
    ThumbRing,
    BrassHorn,
    CrimsonVintage,
    BrandOfOriginalSin,
}

impl Treasure {
    pub const ALL: [Treasure; 4] = [
        Treasure::ThumbRing,
        Treasure::BrassHorn,
        Treasure::CrimsonVintage,
        Treasure::BrandOfOriginalSin,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Treasure::ThumbRing => "Three-Headed Thumb Ring",
            Treasure::BrassHorn => "Brass Horn of Strife",
            Treasure::CrimsonVintage => "Crimson Vintage",
            Treasure::BrandOfOriginalSin => "Brand of Original Sin",
        }
    }
}

/// What a run starts with and keeps: its Rumor and Torture Instruments.
#[derive(Clone, Copy, Debug, Default)]
pub struct Setup {
    /// Opulence: 3 treasure picks per Chromatic instead of 1.
    pub opulence: bool,
    /// Cage Fight: Inmates also eat lower-rarity Inmates they counter, and keep their Code when they eat.
    pub cage_fight: bool,
    /// Tyranny: each stop at the Interrogation Room also sets off Instigate and Bestow 3 more times each.
    pub tyranny: bool,
    pub instruments: Held,
}

/// The run's objective; meeting it gives a Torture Instrument offer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Objective {
    /// Move 18 tiles.
    PrisonWalk { walked: u32 },
    /// An Inmate reaches Ultimate.
    KingOfTheCaged,
    /// Trigger 2 Prison Incidents.
    Undercurrents { triggered: u32 },
    /// Open 10 cells, however they open.
    BehindCellDoors { opened: u32 },
    /// Consume 4 times.
    SurvivalOfTheFittest { consumed: u32 },
}

pub const BEHIND_CELL_DOORS_CELLS: u32 = 10;
pub const SURVIVAL_CONSUMES: u32 = 4;

pub const UNDERCURRENTS_INCIDENTS: u32 = 2;

pub const PRISON_WALK_TILES: u32 = 18;

/// A Lion's Prey mark (Blood-Stained Trap).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Hunt {
    pub lion: usize,
    pub prey: usize,
    pub mornings: u8,
}

#[derive(Clone, Debug)]
pub struct State {
    pub cells: [Cell; CELLS],
    /// The cell index you're in (0-based: cell 1 is 0).
    pub pos: usize,
    /// Days still to play, the current one included.
    pub days_left: u32,
    pub chromatics: u8,
    /// Warden's Treasure picks waiting to be made.
    pub picks: u32,
    pub setup: Setup,
    /// Consumes made today (Barbed Collar).
    pub bites_today: u32,
    pub hunt: Option<Hunt>,
    /// Elephants with a Tusk mark (Iron Tongs of Lust).
    pub tusks: Vec<usize>,
    /// Scratched locked cells and the Inmate behind each (Filthy Claws; only its rarity shows on screen).
    pub scratches: Vec<(usize, Inmate)>,
    /// Squirming Iron Box: Rat nests on locked cells and the mornings until each hatches a Rare Rat.
    pub nests: Vec<(usize, u8)>,
    /// Locked cells whose rarity is already decided (drawn when a card looked behind the doors), and `shown`: those
    /// whose rarity shows (a "?" door: Warden's Scales, Crow Whispers, "Treasure" Hunt). The Code shows only once it
    /// opens.
    pub revealed: Vec<(usize, u8)>,
    pub shown: u16,
    pub objective: Option<Objective>,
    /// Torture Instrument offers waiting to be picked (from a met objective).
    pub offers: u32,
    /// Re-rolls left for a Torture Instrument offer: at most 1 a run, shared by the first and second picks, and
    /// not everyone has it.
    pub rerolls: u32,
    /// Cells holding a Prison Incident (the "!"), a bit per cell: set off by walking in.
    pub incidents: u16,
    /// How far the Usurpation chain has gone (0: Usurpation I is the one in the pool).
    pub usurp: u8,
    /// Chain Reaction: the next move opens every locked cell on the way.
    pub chain: bool,
    /// Sewer Ratway: right now, a jump to any cell but the Interrogation Room (`warp`).
    pub ratway: bool,
}

/// The Prison Incidents (the cards seen in the user's runs).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Incident {
    ChainReaction,
    ExecutionEfficiency,
    BehindClosedDoors,
    RavenousAppetite,
    HighRiskConvict,
    CrowWhispers,
    TasteOfFreedom,
    Usurpation,
    SewerRatway,
    TreasureHunt,
    ExtraPortion,
    TransferOfConvicts,
    Archenemy,
}

impl Incident {
    pub const ALL: [Incident; 13] = [
        Incident::ChainReaction,
        Incident::ExecutionEfficiency,
        Incident::BehindClosedDoors,
        Incident::RavenousAppetite,
        Incident::HighRiskConvict,
        Incident::CrowWhispers,
        Incident::TasteOfFreedom,
        Incident::Usurpation,
        Incident::SewerRatway,
        Incident::TreasureHunt,
        Incident::ExtraPortion,
        Incident::TransferOfConvicts,
        Incident::Archenemy,
    ];
    /// How often each comes up: the cards seen in the user's runs (61), plus one each.
    const WEIGHTS: [f64; 13] = [
        5.0, 6.0, 6.0, 6.0, 4.0, 8.0, 11.0, 10.0, 8.0, 4.0, 2.0, 2.0, 2.0,
    ];

    pub fn draw(rng: &mut Rng) -> Incident {
        let total: f64 = Self::WEIGHTS.iter().sum();
        let mut x = rng.f64() * total;
        for (k, w) in Self::WEIGHTS.iter().enumerate() {
            if x < *w {
                return Self::ALL[k];
            }
            x -= w;
        }
        Incident::TasteOfFreedom
    }
}

/// After an incident is set off, the chance a new one shows on a random cell (community simulator, observed).
const INCIDENT_FOLLOW_UP: f64 = 0.88;

fn draw_rarity(odds: &[f64; 5], min: u8, rng: &mut Rng) -> u8 {
    let total: f64 = odds[min as usize..].iter().sum();
    let mut x = rng.f64() * total;
    for r in min as usize..5 {
        if x < odds[r] {
            return r as u8;
        }
        x -= odds[r];
    }
    min
}

/// Rarity weights for the rest of the board, after Chromatics and Ultimates: higher rarity, bigger chest.
const REST_WEIGHT: [f64; 4] = [1.0, 2.0, 4.0, 8.0];

impl State {
    pub fn new(cells: [Cell; CELLS], pos: usize, days_left: u32, setup: Setup) -> State {
        State {
            cells,
            pos,
            days_left,
            chromatics: 0,
            picks: 0,
            setup,
            bites_today: 0,
            hunt: None,
            tusks: Vec::new(),
            scratches: Vec::new(),
            nests: Vec::new(),
            objective: None,
            offers: 0,
            rerolls: 0,
            incidents: 0,
            usurp: 0,
            chain: false,
            ratway: false,
            revealed: Vec::new(),
            shown: 0,
        }
    }

    /// A run's first day: 4 Inmates out on random cells (3 in about 1 run in 20), in the Interrogation Room.
    pub fn fresh(setup: Setup, rng: &mut Rng) -> State {
        let mut cells = [Cell::Locked; CELLS];
        cells[OFFICE] = Cell::Office;
        // a run lasts 20 days (30 with the Iron Law Rumor; env DAYS for benchmarks)
        let days = std::env::var("DAYS").ok().and_then(|d| d.parse().ok()).unwrap_or(20);
        let mut s = State::new(cells, OFFICE, days, setup);
        let mut free: Vec<usize> = (1..CELLS).collect();
        rng.shuffle(&mut free);
        let n = if rng.chance(0.05) { 3 } else { 4 };
        for &i in &free[..n] {
            s.cells[i] = Cell::Inmate(s.new_door_inmate(rng));
        }
        if rng.chance(0.72) {
            s.place_incidents(1, rng);
        }
        s
    }

    pub fn has_incident(&self, i: usize) -> bool {
        self.incidents & (1 << i) != 0
    }

    /// Puts up to `n` new Prison Incidents on random cells without one: locked cells or cells with an Inmate, not
    /// the Interrogation Room nor the cell you're in.
    pub fn place_incidents(&mut self, n: usize, rng: &mut Rng) {
        for _ in 0..n {
            let free: Vec<usize> = (1..CELLS)
                .filter(|&i| {
                    i != self.pos
                        && !self.has_incident(i)
                        && matches!(self.cells[i], Cell::Locked | Cell::Inmate(_))
                })
                .collect();
            if free.is_empty() {
                return;
            }
            let i = rng.pick(&free);
            self.incidents |= 1 << i;
        }
    }

    /// Instigate: 1 new Prison Incident, 2 on a board with none (community simulator: the user's records).
    pub fn instigate(&mut self, rng: &mut Rng) {
        let n = if self.incidents == 0 { 2 } else { 1 };
        self.place_incidents(n, rng);
    }

    /// A Prison Incident set off in cell `at`: its effect (all taken, as they never hurt), the Undercurrents count,
    /// then most often a new one elsewhere.
    pub fn trigger_incident(&mut self, at: usize, rng: &mut Rng) {
        self.incidents &= !(1 << at);
        if let Some(Objective::Undercurrents { triggered }) = self.objective {
            let triggered = triggered + 1;
            self.objective = if triggered >= UNDERCURRENTS_INCIDENTS {
                self.offers += 1;
                None
            } else {
                Some(Objective::Undercurrents { triggered })
            };
        }
        match Incident::draw(rng) {
            Incident::ChainReaction => self.chain = true,
            Incident::BehindClosedDoors => {
                // the Inmate here becomes an Ultimate of the scarcest Code on the board
                if self.inmate(at).is_some() {
                    let count = |c: Code| {
                        self.inmates()
                            .iter()
                            .filter(|&&i| self.inmate(i).unwrap().code == c)
                            .count()
                    };
                    let code = Code::ALL.into_iter().min_by_key(|&c| count(c)).unwrap();
                    let r = self.inmate(at).unwrap().rarity.max(ULTIMATE);
                    self.unmark(at);
                    self.settle(at, Inmate { code, rarity: r });
                }
            }
            Incident::RavenousAppetite => {
                let all = self.inmates();
                if let Some(low) = all.iter().map(|&i| self.inmate(i).unwrap().rarity).min() {
                    let lows: Vec<usize> = all
                        .into_iter()
                        .filter(|&i| self.inmate(i).unwrap().rarity == low)
                        .collect();
                    let i = rng.pick(&lows);
                    let gained = (0..5).filter(|_| rng.chance(0.5)).count() as u8;
                    self.raise(i, gained);
                }
            }
            Incident::HighRiskConvict => {
                let locked = self.locked();
                if !locked.is_empty() {
                    let i = rng.pick(&locked);
                    self.scratches.retain(|s| s.0 != i);
                    let rarity = draw_rarity(&DOOR_ODDS, LEGENDARY, rng);
                    self.cells[i] = Cell::Inmate(Inmate {
                        code: rng.pick(&Code::ALL),
                        rarity,
                    });
                }
            }
            Incident::TasteOfFreedom => self.open_random(3, rng),
            Incident::Usurpation => {
                // I: 1 Inmate +1, II: 2 Inmates +1, III: 2 Inmates +2, IV: every Inmate +2, then it's gone
                let (count, by) = match self.usurp {
                    0 => (1, 1),
                    1 => (2, 1),
                    2 => (2, 2),
                    3 => (CELLS, 2),
                    _ => (0, 0),
                };
                let mut all = self.inmates();
                rng.shuffle(&mut all);
                for &i in all.iter().take(count) {
                    self.raise(i, by);
                }
                self.usurp = (self.usurp + 1).min(4);
            }
            // the Inmate here eats every lower one it counters, +1 for each
            Incident::ExecutionEfficiency => {
                if let Some(cur) = self.inmate(at) {
                    let prey: Vec<usize> = self
                        .inmates()
                        .into_iter()
                        .filter(|&j| {
                            j != at
                                && self.inmate(j).is_some_and(|p| {
                                    p.rarity < cur.rarity && cur.code.counters(p.code)
                                })
                        })
                        .collect();
                    for &j in &prey {
                        self.remove(j);
                    }
                    self.raise(at, prey.len() as u8);
                }
            }
            // the Inmate here, copied into a random locked cell
            Incident::TransferOfConvicts => {
                if let Some(cur) = self.inmate(at) {
                    let locked = self.locked();
                    if !locked.is_empty() {
                        let j = rng.pick(&locked);
                        self.scratches.retain(|s| s.0 != j);
                        self.revealed.retain(|s| s.0 != j);
                        self.cells[j] = Cell::Inmate(cur);
                    }
                }
            }
            // a random locked cell opens on a meal for the Inmate here
            Incident::Archenemy => {
                if let Some(cur) = self.inmate(at) {
                    let foods: Vec<Code> = Code::ALL
                        .into_iter()
                        .filter(|&c| {
                            self.can_eat(
                                cur.code,
                                cur.rarity,
                                Inmate {
                                    code: c,
                                    rarity: cur.rarity,
                                },
                            )
                        })
                        .collect();
                    let locked = self.locked();
                    if !locked.is_empty() && !foods.is_empty() {
                        let j = rng.pick(&locked);
                        self.scratches.retain(|s| s.0 != j);
                        self.revealed.retain(|s| s.0 != j);
                        self.cells[j] = Cell::Inmate(Inmate {
                            code: rng.pick(&foods),
                            rarity: cur.rarity,
                        });
                    }
                }
            }
            Incident::ExtraPortion => {
                let mut all = self.inmates();
                rng.shuffle(&mut all);
                for &i in all.iter().take(3) {
                    self.raise(i, 1);
                }
            }
            // what they show isn't played: no effect on the board
            // Crow Whispers: the rarity behind 4 locked cells; "Treasure" Hunt: every locked Normal or Magic shows
            Incident::CrowWhispers => self.reveal(rng, |h| h.iter().take(4).map(|x| x.0).collect()),
            Incident::TreasureHunt => self.reveal(rng, |h| {
                h.iter().filter(|x| x.1 <= MAGIC).map(|x| x.0).collect()
            }),
            // a jump to any cell, chosen by the player (the policy, in a future)
            Incident::SewerRatway => self.ratway = true,
        }
        if rng.chance(INCIDENT_FOLLOW_UP) {
            self.place_incidents(1, rng);
        }
    }

    pub fn has(&self, i: crate::instruments::Instrument) -> bool {
        self.setup.instruments.has(i)
    }

    pub fn inmate(&self, i: usize) -> Option<Inmate> {
        match self.cells[i] {
            Cell::Inmate(m) => Some(m),
            _ => None,
        }
    }

    pub fn inmates(&self) -> Vec<usize> {
        (0..CELLS).filter(|&i| self.inmate(i).is_some()).collect()
    }

    pub fn locked(&self) -> Vec<usize> {
        (0..CELLS)
            .filter(|&i| self.cells[i] == Cell::Locked)
            .collect()
    }

    /// The rarity of a "?" door (shown on screen).
    pub fn revealed_at(&self, i: usize) -> Option<u8> {
        if self.shown & (1 << i) == 0 {
            return None;
        }
        self.decided_at(i)
    }

    fn decided_at(&self, i: usize) -> Option<u8> {
        self.revealed.iter().find(|r| r.0 == i).map(|r| r.1)
    }

    /// Puts a rarity shown on screen behind a locked cell.
    pub fn show(&mut self, i: usize, rarity: u8) {
        self.revealed.retain(|r| r.0 != i);
        self.revealed.push((i, rarity));
        self.shown |= 1 << i;
    }

    /// Decides what's behind locked cells not known yet, and shows their rarity: `which` picks among the cells
    /// with their rarities (as the card that reveals them does).
    fn reveal(&mut self, rng: &mut Rng, which: impl Fn(&[(usize, u8)]) -> Vec<usize>) {
        let mut locked = self.locked();
        // clockwise from the warden (Crow Whispers shows the next ones)
        locked.sort_by_key(|&i| (i + CELLS - self.pos) % CELLS);
        // what's behind every locked cell gets decided now (what isn't shown is then known by its absence too)
        for &i in &locked {
            if self.decided_at(i).is_none() && self.scratch_at(i).is_none() {
                let r = self.new_door_inmate(rng).rarity;
                self.revealed.push((i, r));
            }
        }
        let hidden: Vec<(usize, u8)> = locked
            .into_iter()
            .filter(|&i| self.scratch_at(i).is_none() && self.shown & (1 << i) == 0)
            .filter_map(|i| self.decided_at(i).map(|r| (i, r)))
            .collect();
        for i in which(&hidden) {
            self.shown |= 1 << i;
        }
    }

    fn scratch_at(&self, i: usize) -> Option<Inmate> {
        self.scratches.iter().find(|s| s.0 == i).map(|s| s.1)
    }

    /// Whether an eater of this Code and rarity can Consume `prey`, with the instruments held.
    fn can_eat(&self, code: Code, rarity: u8, prey: Inmate) -> bool {
        if prey.rarity == rarity {
            if code.counters(prey.code)
                || (code == Code::Lion && self.has(IronCrown))
                || (code == Code::Rat && prey.code == Code::Rat && self.has(SkeweredRatHeads))
            {
                return true;
            }
        }
        (self.setup.cage_fight && code.counters(prey.code) && prey.rarity < rarity)
            || (code == Code::Elephant
                && code.counters(prey.code)
                && prey.rarity > rarity
                && self.has(ElephantFootMaul))
            || (code == Code::Wolf
                && prey.code == Code::Rat
                && prey.rarity < rarity
                && self.has(FangedGuillotine))
    }

    /// The cells an Inmate of this Code and rarity in cell `i` could Consume: Inmates, and with Filthy Claws the
    /// scratched cells of its rarity for a Wolf. At the Chromatic cap an Ultimate has none, since it couldn't rise.
    fn targets_as(&self, i: usize, code: Code, rarity: u8) -> Vec<usize> {
        if rarity >= CHROMATIC || (rarity == ULTIMATE && self.chromatics >= MAX_CHROMATICS) {
            return Vec::new();
        }
        let mut t: Vec<usize> = (0..CELLS)
            .filter(|&j| {
                j != i && matches!(self.cells[j], Cell::Inmate(p) if self.can_eat(code, rarity, p))
            })
            .collect();
        if code == Code::Wolf && self.has(FilthyClaws) {
            t.extend(
                self.scratches
                    .iter()
                    .filter(|s| s.1.rarity == rarity && self.cells[s.0] == Cell::Locked)
                    .map(|s| s.0),
            );
        }
        t
    }

    /// Whether the Inmate in cell `i` has anything to eat (as `targets`, without listing them).
    pub fn has_target(&self, i: usize) -> bool {
        let Some(e) = self.inmate(i) else { return false };
        if e.rarity >= CHROMATIC || (e.rarity == ULTIMATE && self.chromatics >= MAX_CHROMATICS) {
            return false;
        }
        (0..CELLS).any(|j| j != i && matches!(self.cells[j], Cell::Inmate(p) if self.can_eat(e.code, e.rarity, p)))
            || (e.code == Code::Wolf
                && self.has(FilthyClaws)
                && self
                    .scratches
                    .iter()
                    .any(|s| s.1.rarity == e.rarity && self.cells[s.0] == Cell::Locked))
    }

    /// The cells the Inmate in cell `i` can Consume now (the red jaws when not empty).
    pub fn targets(&self, i: usize) -> Vec<usize> {
        match self.inmate(i) {
            Some(e) => self.targets_as(i, e.code, e.rarity),
            None => Vec::new(),
        }
    }

    /// Forgets the marks on a cell whose Inmate is gone or changed.
    fn unmark(&mut self, i: usize) {
        self.tusks.retain(|&t| t != i);
        if self.hunt.is_some_and(|h| h.lion == i || h.prey == i) {
            self.hunt = None;
        }
    }

    /// An Inmate leaves the board; its cell locks again.
    pub fn remove(&mut self, i: usize) {
        self.cells[i] = Cell::Locked;
        self.unmark(i);
    }

    /// Puts an Inmate in cell `i` at its new rarity: a Chromatic leaves the board (its cell locks again) and gives
    /// its treasure picks; past the cap it stays Ultimate.
    fn settle(&mut self, i: usize, mut m: Inmate) {
        if m.rarity >= ULTIMATE && self.objective == Some(Objective::KingOfTheCaged) {
            self.objective = None;
            self.offers += 1;
        }
        if m.rarity >= CHROMATIC {
            if self.chromatics < MAX_CHROMATICS {
                self.chromatics += 1;
                self.remove(i);
                self.picks += if self.setup.opulence { 3 } else { 1 };
                return;
            }
            m.rarity = ULTIMATE;
        }
        self.cells[i] = Cell::Inmate(m);
    }

    pub fn raise(&mut self, i: usize, tiers: u8) {
        if let Some(mut m) = self.inmate(i) {
            m.rarity = (m.rarity + tiers).min(CHROMATIC);
            self.settle(i, m);
        }
    }

    fn raise_random(&mut self, n: usize, rng: &mut Rng) {
        for _ in 0..n {
            let all = self.inmates();
            if all.is_empty() {
                return;
            }
            let i = rng.pick(&all);
            self.raise(i, 1);
        }
    }

    fn new_door_inmate(&self, rng: &mut Rng) -> Inmate {
        let min = if self.has(SortingHook) { MAGIC } else { NORMAL };
        Inmate {
            code: rng.pick(&Code::ALL),
            rarity: draw_rarity(&DOOR_ODDS, min, rng),
        }
    }

    /// Opens a locked cell. `patrol`: opened by moving there, which sets off the instruments reacting to openings.
    pub fn open(&mut self, i: usize, rng: &mut Rng, patrol: bool) {
        if self.cells[i] != Cell::Locked {
            return;
        }
        let m = match (self.scratch_at(i), self.decided_at(i)) {
            (Some(m), _) => m,
            (None, Some(r)) => Inmate {
                code: rng.pick(&Code::ALL),
                rarity: r,
            },
            (None, None) => self.new_door_inmate(rng),
        };
        self.scratches.retain(|s| s.0 != i);
        self.revealed.retain(|s| s.0 != i);
        self.nests.retain(|n| n.0 != i);
        self.shown &= !(1 << i);
        self.cells[i] = Cell::Inmate(m);
        if let Some(Objective::BehindCellDoors { opened }) = self.objective {
            self.objective = if opened + 1 >= BEHIND_CELL_DOORS_CELLS {
                self.offers += 1;
                None
            } else {
                Some(Objective::BehindCellDoors { opened: opened + 1 })
            };
        }
        if !patrol {
            return;
        }
        if self.has(FelonsDogTag) && rng.chance(0.25) {
            self.raise(i, 1);
        }
        if self.has(SearingBrandingIron) && rng.chance(0.35) {
            if let Some(m) = self.inmate(i) {
                let locked = self.locked();
                if !locked.is_empty() {
                    let j = rng.pick(&locked);
                    self.scratches.retain(|s| s.0 != j);
                    self.cells[j] = Cell::Inmate(m);
                }
            }
        }
        if self.has(LostRingOfKeys) && rng.chance(0.8) {
            self.open((i + CELLS - 1) % CELLS, rng, false);
            self.open((i + 1) % CELLS, rng, false);
        }
        if self.has(ForgedKey) && rng.chance(0.5) {
            self.open_random(1, rng);
        }
        if self.has(ExecutionersBlacklist) && self.locked().len() <= 5 {
            self.raise_random(2, rng);
        }
    }

    /// Opens up to `n` random locked cells, setting nothing off.
    pub fn open_random(&mut self, n: usize, rng: &mut Rng) {
        for _ in 0..n {
            let locked = self.locked();
            if locked.is_empty() {
                return;
            }
            let i = rng.pick(&locked);
            self.open(i, rng, false);
        }
    }

    /// What happens at the start of a day, before the destinations: the instruments acting each morning.
    pub fn morning(&mut self, rng: &mut Rng) {
        self.bites_today = 0;
        if self.has(RavenousChains) {
            let all = self.inmates();
            if let Some(top) = all.iter().map(|&i| self.inmate(i).unwrap().rarity).max() {
                let tops: Vec<usize> = all
                    .into_iter()
                    .filter(|&i| self.inmate(i).unwrap().rarity == top)
                    .collect();
                let i = rng.pick(&tops);
                let mut gained = 0;
                for n in [(i + CELLS - 1) % CELLS, (i + 1) % CELLS] {
                    if self.inmate(n).is_some() {
                        self.remove(n);
                        if rng.chance(0.4) {
                            gained += 1;
                        }
                    }
                }
                self.raise(i, gained);
            }
        }
        if self.has(KinsCoinOfFate) {
            let all = self.inmates();
            let pairs: Vec<(usize, usize)> = all
                .iter()
                .flat_map(|&a| all.iter().map(move |&b| (a, b)))
                .filter(|&(a, b)| a != b && self.inmate(a) == self.inmate(b))
                .collect();
            if !pairs.is_empty() && rng.chance(0.6) {
                let (gone, up) = rng.pick(&pairs);
                self.remove(gone);
                self.raise(up, 2);
            }
        }
        if let Some(mut h) = self.hunt {
            if self.inmate(h.lion).is_some() && self.inmate(h.prey).is_some() && h.mornings > 0 {
                h.mornings -= 1;
                self.hunt = if h.mornings > 0 { Some(h) } else { None };
                // the prey first, so a Lion leaving as a Chromatic doesn't leave the prey behind un-raised
                self.raise(h.prey, 1);
                self.raise(h.lion, 1);
            } else {
                self.hunt = None;
            }
        }
        if self.has(IronTongsOfLust) {
            let untusked: Vec<usize> = self
                .inmates()
                .into_iter()
                .filter(|&i| {
                    self.inmate(i).unwrap().code == Code::Elephant && !self.tusks.contains(&i)
                })
                .collect();
            if !untusked.is_empty() {
                let i = rng.pick(&untusked);
                self.tusks.push(i);
            }
        }
        if self.has(SquirmingIronBox) {
            // the nests age; one that's done hatches a Rare Rat (community simulator: 2 days later)
            let mut hatched = Vec::new();
            for n in self.nests.iter_mut() {
                n.1 = n.1.saturating_sub(1);
                if n.1 == 0 {
                    hatched.push(n.0);
                }
            }
            self.nests.retain(|n| n.1 > 0);
            for i in hatched {
                if self.cells[i] == Cell::Locked {
                    self.settle(i, Inmate { code: Code::Rat, rarity: RARE });
                }
            }
            let fresh: Vec<usize> = self
                .locked()
                .into_iter()
                .filter(|&i| !self.nests.iter().any(|n| n.0 == i) && self.scratch_at(i).is_none())
                .collect();
            if !fresh.is_empty() {
                let i = rng.pick(&fresh);
                self.nests.push((i, 2));
            }
        }
        if self.has(FilthyClaws) {
            let fresh: Vec<usize> = self
                .locked()
                .into_iter()
                .filter(|&i| self.scratch_at(i).is_none())
                .collect();
            if !fresh.is_empty() {
                let i = rng.pick(&fresh);
                let m = self.new_door_inmate(rng);
                self.scratches.push((i, m));
            }
        }
    }

    pub fn destination_count(&self) -> usize {
        let fewer = [ForgedKey, BurningStake, BarbedWhip]
            .into_iter()
            .filter(|&i| self.has(i))
            .count();
        3usize.saturating_sub(fewer).max(1)
    }

    /// The day's destinations: different cells among the next 6 clockwise, nearest first. The Interrogation Room
    /// is offered at about 60% of another cell's odds (the user's runs: offered 48 times in 127 when in reach).
    pub fn destinations(&self, rng: &mut Rng) -> Vec<usize> {
        let mut pool: Vec<(usize, f64)> = (1..=6)
            .map(|d| ((self.pos + d) % CELLS, 1.0))
            .map(|(c, w)| (c, if c == OFFICE { 0.6 } else { w }))
            .collect();
        let mut out = Vec::new();
        while out.len() < self.destination_count() && !pool.is_empty() {
            let total: f64 = pool.iter().map(|p| p.1).sum();
            let mut x = rng.f64() * total;
            let mut k = pool.len() - 1;
            for (n, p) in pool.iter().enumerate() {
                if x < p.1 {
                    k = n;
                    break;
                }
                x -= p.1;
            }
            out.push(pool.remove(k).0);
        }
        out.sort_by_key(|&c| (c + CELLS - self.pos) % CELLS);
        out
    }

    /// Moving to cell `i`, opening it if it's locked.
    pub fn go(&mut self, i: usize, rng: &mut Rng) {
        let steps = (i + CELLS - self.pos) % CELLS;
        self.pos = i;
        if let Some(Objective::PrisonWalk { walked }) = self.objective {
            let walked = walked + steps as u32;
            self.objective = if walked >= PRISON_WALK_TILES {
                self.offers += 1;
                None
            } else {
                Some(Objective::PrisonWalk { walked })
            };
        }
        if self.chain {
            self.chain = false;
            for d in 1..=steps {
                self.open((i + CELLS - steps + d) % CELLS, rng, d == steps);
            }
        } else {
            self.open(i, rng, true);
        }
        if i == OFFICE && self.setup.tyranny {
            for _ in 0..3 {
                self.instigate(rng);
                self.bestow(rng);
            }
        }
        if self.has_incident(i) && self.inmate(i).is_some() {
            self.trigger_incident(i, rng);
        }
        if self.has(LongChainedShackles) && steps >= 4 && rng.chance(0.4) {
            self.raise_random(1, rng);
        }
        if self.has(ShortChainedShackles) && steps <= 3 && rng.chance(0.5) {
            self.open_random(1, rng);
        }
    }

    /// Sewer Ratway's jump: straight to cell `i` (no walk: nothing on the way, no walk effects), which opens and sets
    /// off its Prison Incident like an arrival.
    pub fn warp(&mut self, i: usize, rng: &mut Rng) {
        self.ratway = false;
        self.pos = i;
        self.open(i, rng, true);
        if self.has_incident(i) && self.inmate(i).is_some() {
            self.trigger_incident(i, rng);
        }
    }

    /// The cells Sewer Ratway can jump to.
    pub fn warp_options(&self) -> Vec<usize> {
        (1..CELLS).filter(|&c| c != self.pos).collect()
    }

    /// The Inmate in cell `eater` Consumes the one in `target`: up a tier (more with some instruments) with a
    /// random Code; the eaten cell locks.
    pub fn consume(&mut self, eater: usize, target: usize, rng: &mut Rng) {
        if let Some(Objective::SurvivalOfTheFittest { consumed }) = self.objective {
            self.objective = if consumed + 1 >= SURVIVAL_CONSUMES {
                self.offers += 1;
                None
            } else {
                Some(Objective::SurvivalOfTheFittest {
                    consumed: consumed + 1,
                })
            };
        }
        let Some(e) = self.inmate(eater) else { return };
        let scratched = self.cells[target] == Cell::Locked;
        let meal = if scratched {
            self.scratch_at(target)
        } else {
            self.inmate(target)
        };
        let Some(meal) = meal else { return };
        let on_board = self.inmates().len();
        let mut rarity = e.rarity + 1;
        if scratched {
            rarity = e.rarity + 3;
        } else if e.code == Code::Lion && self.has(IronCrown) {
            rarity = e.rarity + 3;
        } else if e.code == Code::Elephant && self.has(ElephantFootMaul) {
            rarity = meal.rarity + 2;
        }
        if e.code == Code::Rat && self.has(SkeweredRatHeads) {
            rarity += 2;
        }
        if self.has(AgonizingMuzzle) && on_board <= 4 {
            rarity += 2;
        }
        if self.has(BarbedCollar) && self.bites_today >= 1 && rng.chance(0.8) {
            rarity += 1;
        }
        if self.has(RustySaw) && rng.chance(0.25) {
            rarity += 1;
        }
        if self.has(BarbedWhip) && rng.chance(0.35) {
            rarity += 1;
        }
        self.bites_today += 1;
        if scratched {
            self.scratches.retain(|s| s.0 != target);
        } else {
            self.remove(target);
        }
        self.unmark(eater);
        let code = if self.setup.cage_fight {
            e.code
        } else {
            rng.pick(&Code::ALL)
        };
        let new = Inmate {
            code,
            rarity: rarity.min(CHROMATIC),
        };
        self.settle(eater, new);
        if self.has(CrowdedBeastcage) && rng.chance(0.4) {
            self.open(target, rng, false);
        }
        if self.has(AvengersCasket) && rng.chance(0.3) {
            if let Some(e) = self.inmate(eater) {
                let foods: Vec<Code> = Code::ALL
                    .into_iter()
                    .filter(|&c| {
                        self.can_eat(
                            e.code,
                            e.rarity,
                            Inmate {
                                code: c,
                                rarity: e.rarity,
                            },
                        )
                    })
                    .collect();
                let locked = self.locked();
                if !foods.is_empty() && !locked.is_empty() && e.rarity < CHROMATIC {
                    let j = rng.pick(&locked);
                    self.scratches.retain(|s| s.0 != j);
                    self.cells[j] = Cell::Inmate(Inmate {
                        code: rng.pick(&foods),
                        rarity: e.rarity,
                    });
                }
            }
        }
    }

    /// A new Inmate in cell `i`: a random rarity and a Code different from the one replaced.
    pub fn replace(&mut self, i: usize, rng: &mut Rng) {
        let Some(old) = self.inmate(i) else { return };
        let others: Vec<usize> = self.inmates().into_iter().filter(|&j| j != i).collect();
        let above = others
            .iter()
            .filter(|&&j| self.inmate(j).unwrap().rarity > old.rarity)
            .count();
        let below = others
            .iter()
            .filter(|&&j| self.inmate(j).unwrap().rarity < old.rarity)
            .count();
        let tusked = self.tusks.contains(&i) && old.code == Code::Elephant;
        self.unmark(i);
        let mut rarity = draw_rarity(
            &REPLACE_ODDS,
            if self.has(HangmansNoose) {
                RARE
            } else {
                NORMAL
            },
            rng,
        );
        if self.has(BurningStake) && old.rarity < LEGENDARY && rng.chance(0.4) {
            rarity = rarity.max(LEGENDARY);
        }
        if self.has(ScapegoatsGuillotine) && others.len() >= 2 {
            let mut o = others.clone();
            rng.shuffle(&mut o);
            for &j in &o[..2] {
                self.remove(j);
            }
            rarity = rarity.max(old.rarity + 1);
        }
        // the newcomer gains 3 tiers (card: "+3 additional Rarity Tiers"; community simulator likewise)
        if self.has(SeveredIronMaiden) && above == below {
            rarity = (rarity + 3).min(CHROMATIC);
        }
        let code = if self.has(IronPestle) {
            old.code
        } else {
            let codes: Vec<Code> = Code::ALL.into_iter().filter(|&c| c != old.code).collect();
            rng.pick(&codes)
        };
        if tusked && self.has(IronTongsOfLust) {
            let mut o: Vec<usize> = self.inmates().into_iter().filter(|&j| j != i).collect();
            rng.shuffle(&mut o);
            for &j in o.iter().take(4) {
                self.raise(j, 1);
            }
        }
        self.settle(
            i,
            Inmate {
                code,
                rarity: rarity.min(CHROMATIC),
            },
        );
        // Warden's Scales: 80%, the highest rarity behind the locked cells shows
        if self.has(WardensScales) && rng.chance(0.8) {
            self.reveal(rng, |h| {
                h.iter()
                    .max_by_key(|x| x.1)
                    .map(|x| vec![x.0])
                    .unwrap_or_default()
            });
        }
        if self.has(GaolersExecutionAxe) && rarity > old.rarity {
            self.open_random(2, rng);
        }
    }

    /// Blood-Stained Trap: the Lion in the current cell puts a Prey mark on one of its targets.
    pub fn start_hunt(&mut self, prey: usize) {
        self.hunt = Some(Hunt {
            lion: self.pos,
            prey,
            mornings: HUNT_MORNINGS,
        });
    }

    /// Whether Hunt is a choice in the current cell.
    pub fn can_hunt(&self) -> bool {
        self.has(BloodStainedTrap)
            && self.hunt.is_none()
            && self.inmate(self.pos).is_some_and(|m| m.code == Code::Lion)
    }

    /// Three different Torture Instruments not held yet, the Code-named ones 1/8 as likely (community simulator).
    pub fn instrument_offer(&self, rng: &mut Rng) -> Vec<crate::instruments::Instrument> {
        let mut pool: Vec<(crate::instruments::Instrument, f64)> = crate::instruments::ALL
            .into_iter()
            .filter(|&i| !self.has(i))
            .map(|i| (i, if i.code_named() { 0.125 } else { 1.0 }))
            .collect();
        let mut out = Vec::new();
        while out.len() < 3 && !pool.is_empty() {
            let total: f64 = pool.iter().map(|p| p.1).sum();
            let mut x = rng.f64() * total;
            let mut k = pool.len() - 1;
            for (n, p) in pool.iter().enumerate() {
                if x < p.1 {
                    k = n;
                    break;
                }
                x -= p.1;
            }
            out.push(pool.remove(k).0);
        }
        out
    }

    /// Bestow: a random Inmate goes up one tier.
    pub fn bestow(&mut self, rng: &mut Rng) {
        self.raise_random(1, rng);
    }

    pub fn treasure(&mut self, t: Treasure, rng: &mut Rng) {
        self.picks = self.picks.saturating_sub(1);
        match t {
            Treasure::ThumbRing => self.open_random(3, rng),
            Treasure::BrassHorn => {
                // In cell order; an Inmate eaten earlier in the round no longer takes its turn.
                for i in 0..CELLS {
                    let t: Vec<usize> = self
                        .targets(i)
                        .into_iter()
                        .filter(|&j| self.inmate(j).is_some())
                        .collect();
                    if !t.is_empty() {
                        let j = rng.pick(&t);
                        self.consume(i, j, rng);
                    }
                }
            }
            Treasure::CrimsonVintage => {
                for i in 0..CELLS {
                    if let Some(m) = self.inmate(i) {
                        if m.rarity < LEGENDARY {
                            self.raise(i, 1);
                        }
                    }
                }
            }
            Treasure::BrandOfOriginalSin => {
                let mut without: Vec<usize> = self
                    .inmates()
                    .into_iter()
                    .filter(|&i| !self.has_target(i))
                    .collect();
                rng.shuffle(&mut without);
                for i in without.into_iter().take(4) {
                    let Some(m) = self.inmate(i) else { continue };
                    let codes: Vec<Code> = Code::ALL
                        .into_iter()
                        .filter(|&c| !self.targets_as(i, c, m.rarity).is_empty())
                        .collect();
                    if !codes.is_empty() {
                        self.unmark(i);
                        self.cells[i] = Cell::Inmate(Inmate {
                            code: rng.pick(&codes),
                            rarity: m.rarity,
                        });
                    }
                }
            }
        }
    }

    pub fn ultimates(&self) -> usize {
        self.inmates()
            .into_iter()
            .filter(|&i| self.inmate(i).unwrap().rarity == ULTIMATE)
            .count()
    }

    /// How good a finished run is: Chromatics first, then Ultimates on the board, then the rest by rarity.
    pub fn score(&self) -> f64 {
        let rest: f64 = self
            .inmates()
            .into_iter()
            .map(|i| self.inmate(i).unwrap().rarity)
            .filter(|&r| r < ULTIMATE)
            .map(|r| REST_WEIGHT[r as usize])
            .sum();
        // What the player wants: the 10 Chromatics (worth 3 more on top of their count, so a better chance at 10
        // beats a slightly higher average below it), then, once there, as many Ultimates on the board as possible.
        // Below 10 the count still tells how close the run came; the rest of the board only breaks ties.
        if self.chromatics >= MAX_CHROMATICS {
            self.chromatics as f64 * 100_000.0
                + 300_000.0
                + self.ultimates() as f64 * 1_000.0
                + rest
        } else {
            self.chromatics as f64 * 100_000.0 + self.ultimates() as f64 * 10.0 + rest * 0.01
        }
    }
}
