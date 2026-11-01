//! From a picture of the game and what the log said, to advice and the boxes to draw. No Windows here, so it's
//! tested on recorded frames; the overlay (src/bin/tartarus-overlay.rs) only captures, draws and talks.
//!
//! What the screen can't show is kept in `RunState`, saved to a file after every change, so the overlay can be
//! closed and started again mid-run (after an update, say) and carry on.

use crate::game::*;
use crate::instruments::Instrument;
use crate::reader::{self, BoardRead, CellRead, Frame, Label};
use crate::solver::{self, Action, Phase};
use serde_json::{json, Value};

/// Which pop-up the log says is open over the board.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Popup {
    None,
    Instrument,
    Treasure,
    Incident,
    Rumor,
    Objective,
    End,
}

/// A rectangle on the 1920x1080 reference picture (or, once mapped by the view, on the game window).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

const fn r(x0: i32, y0: i32, x1: i32, y1: i32) -> Rect {
    Rect { x0, y0, x1, y1 }
}

/// The bottom cards and buttons, by how many there are (measured on 1080p screenshots; 2 cards is a guess for the
/// destination cards, measured for the decision buttons).
fn bottom(count: usize, k: usize) -> Rect {
    match count {
        4 => [
            r(278, 745, 608, 924),
            r(623, 745, 953, 924),
            r(968, 745, 1298, 924),
            r(1313, 745, 1643, 924),
        ][k.min(3)],
        3 => [
            r(450, 745, 781, 924),
            r(795, 745, 1126, 924),
            r(1140, 745, 1471, 924),
        ][k.min(2)],
        2 => [r(622, 745, 953, 924), r(967, 745, 1298, 924)][k.min(1)],
        _ => r(795, 745, 1126, 924),
    }
}

const INSTRUMENT_CARDS: [Rect; 3] = [
    r(245, 180, 708, 875),
    r(725, 180, 1188, 875),
    r(1205, 180, 1668, 875),
];
const REROLL: Rect = r(648, 922, 924, 998);
const TREASURE_CARDS: [Rect; 4] = [
    r(100, 190, 520, 850),
    r(540, 190, 960, 850),
    r(980, 190, 1400, 850),
    r(1420, 190, 1830, 850),
];
/// The eye at the top right of the treasure and instrument screens (View Prison Status / Return to Selection).
const EYE: Rect = r(1798, 96, 1876, 174);
const CANCEL: Rect = r(1652, 732, 1792, 872);

fn cell_box(n: usize) -> Rect {
    let (x, y) = CELL_XY[n];
    r(x - 56, y - 66, x + 56, y + 62)
}

/// The cells' portrait centres (data/layout-1080p.json).
const CELL_XY: [(i32, i32); CELLS] = [
    (520, 135),
    (698, 135),
    (871, 135),
    (1045, 135),
    (1220, 135),
    (1393, 135),
    (1400, 270),
    (1408, 411),
    (1417, 560),
    (1233, 560),
    (1052, 560),
    (868, 560),
    (686, 560),
    (503, 560),
    (512, 415),
    (515, 273),
];

/// What the screen can't show, kept between pictures and between starts of the overlay.
#[derive(Clone, Debug, Default)]
pub struct RunState {
    /// The last offer's names by slot, to fill a card the reader misses on the next picture of the same offer.
    pub last_offer: Vec<Option<String>>,
    pub rumor: Option<String>,
    pub instruments: Vec<String>,
    /// "prison-walk", "king-of-the-caged", or another name the engine doesn't play.
    pub objective: Option<String>,
    pub walked: u32,
    /// From the log: days done and the run's length.
    pub day: Option<(u32, u32)>,
    /// The last whole board read (not under a pop-up or the Consume screen's red).
    pub board: Option<SavedBoard>,
    /// The eater's rarity when Consume was chosen (to count the bites of a chain).
    pub eater_rarity: Option<u8>,
    /// Warden's Treasure picks confirmed since the last board.
    pub picks_done: u32,
    /// The instrument card selected on the offer screen (its name).
    pub selected_instrument: Option<String>,
    /// The warden's cell at the last destination screen (for Prison Walk).
    pub last_warden: Option<usize>,
    /// Prison Incidents set off this run (for Undercurrents).
    pub triggered: u32,
    /// The cells with a Prison Incident's "!" on the last board read, a bit per cell.
    pub incidents: u16,
    /// Cells opened this run (Behind Cell Doors), from locked cells found open on the next board.
    pub opened: u32,
    /// Consumes this run (Survival of the Fittest), counted from the log (`objective survival <n>` corrects it).
    pub consumed: u32,
    /// The saved board was read while the current pop-up was open (cards hidden).
    pub board_fresh: bool,
    /// `selected_instrument` is the advised card, not one seen selected.
    pub selected_guess: bool,
    /// The "?" doors on the last board read: locked cells and the rarity behind them.
    pub revealed: Vec<(usize, u8)>,
    /// The Usurpation that comes next (0: I, ... 4: all four seen), from the incident cards read.
    pub usurp: u8,
    /// Chain Reaction is waiting for the next move: the day it was set off.
    pub chain_at: Option<u32>,
    /// The screen and day the saved board was read on (for telling what changed since).
    pub board_screen: Option<&'static str>,
    pub board_day: Option<u32>,
    /// The last screen advised that the player answers (a destination, a decision, an instrument): what the box was
    /// on, to compare with what the player did once the next screen shows it.
    pub pending: Option<Pending>,
    /// The last screen recorded in the journal (one record per screen).
    pub journal_key: String,
    /// The run's end was recorded.
    pub ended: bool,
    /// The most Chromatics seen this run.
    pub best_chromatics: u8,
}

/// A choice the box made, waiting for the next screen to show what the player did.
#[derive(Clone, Debug)]
pub struct Pending {
    pub screen: String,
    pub boxed: String,
    /// The options as (label, expected Chromatics).
    pub options: Vec<(String, f64)>,
    pub day: Option<u32>,
    pub warden: Option<usize>,
    pub cells: Option<[Cell; CELLS]>,
    pub instruments: usize,
}

#[derive(Clone, Debug)]
pub struct SavedBoard {
    pub cells: [Cell; CELLS],
    pub warden: usize,
    pub chromatics: u8,
}

fn cell_text(c: &Cell) -> String {
    match c {
        Cell::Locked => "locked".into(),
        Cell::Empty => "empty".into(),
        Cell::Office => "office".into(),
        Cell::Inmate(m) => format!(
            "{} {}",
            m.code.name().to_lowercase(),
            ["N", "M", "R", "L", "U", "C"][m.rarity as usize]
        ),
    }
}

impl RunState {
    pub fn to_json(&self) -> Value {
        json!({
            "rumor": self.rumor, "instruments": self.instruments, "objective": self.objective, "walked": self.walked,
            "day": self.day.map(|(c, m)| vec![c, m]),
            "board": self.board.as_ref().map(|b| json!({
                "cells": b.cells.iter().map(cell_text).collect::<Vec<_>>(), "warden": b.warden, "chromatics": b.chromatics,
            })),
            "last_warden": self.last_warden, "triggered": self.triggered, "incidents": self.incidents, "opened": self.opened, "consumed": self.consumed, "usurp": self.usurp, "chain_at": self.chain_at,
        })
    }

    pub fn from_json(v: &Value) -> RunState {
        let mut s = RunState {
            rumor: v["rumor"].as_str().map(String::from),
            instruments: v["instruments"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            objective: v["objective"].as_str().map(String::from),
            walked: v["walked"].as_u64().unwrap_or(0) as u32,
            day: v["day"]
                .as_array()
                .and_then(|a| Some((a.first()?.as_u64()? as u32, a.get(1)?.as_u64()? as u32))),
            last_warden: v["last_warden"].as_u64().map(|x| x as usize),
            triggered: v["triggered"].as_u64().unwrap_or(0) as u32,
            incidents: v["incidents"].as_u64().unwrap_or(0) as u16,
            opened: v["opened"].as_u64().unwrap_or(0) as u32,
            consumed: v["consumed"].as_u64().unwrap_or(0) as u32,
            usurp: v["usurp"].as_u64().unwrap_or(0) as u8,
            chain_at: v["chain_at"].as_u64().map(|x| x as u32),
            ..Default::default()
        };
        if let Some(b) = v["board"].as_object() {
            let mut cells = [Cell::Locked; CELLS];
            if let Some(a) = b.get("cells").and_then(|x| x.as_array()) {
                for (n, c) in a.iter().enumerate().take(CELLS) {
                    cells[n] = parse_cell(c.as_str().unwrap_or("locked"));
                }
            }
            s.board = Some(SavedBoard {
                cells,
                warden: b.get("warden").and_then(|x| x.as_u64()).unwrap_or(0) as usize,
                chromatics: b.get("chromatics").and_then(|x| x.as_u64()).unwrap_or(0) as u8,
            });
        }
        s
    }

    /// A new run: forget what the last one held.
    pub fn new_run(&mut self, max: u32) {
        *self = RunState {
            day: Some((0, max)),
            ..Default::default()
        };
    }

    pub fn held(&self) -> Vec<Instrument> {
        self.instruments
            .iter()
            .filter_map(|n| Instrument::find(n))
            .collect()
    }

    fn setup(&self) -> Setup {
        let mut s = Setup::default();
        if let Some(r) = &self.rumor {
            let k = crate::instruments::slug(r);
            s.opulence = k == "opulence";
            s.cage_fight = k == "cage-fight";
            s.tyranny = k == "tyranny";
        }
        for i in self.held() {
            s.instruments.add(i);
        }
        s
    }

    fn days_left(&self) -> u32 {
        match self.day {
            Some((cur, max)) => max.saturating_sub(cur) + 1,
            None => 10,
        }
    }

    pub fn state(&self, cells: [Cell; CELLS], warden: usize, chromatics: u8) -> State {
        let mut s = State::new(cells, warden, self.days_left(), self.setup());
        s.chromatics = chromatics.min(MAX_CHROMATICS);
        s.objective = match self.objective.as_deref() {
            Some("prison-walk") => Some(Objective::PrisonWalk {
                walked: self.walked,
            }),
            Some("king-of-the-caged") => Some(Objective::KingOfTheCaged),
            Some("undercurrents") if self.triggered < UNDERCURRENTS_INCIDENTS => {
                Some(Objective::Undercurrents {
                    triggered: self.triggered,
                })
            }
            Some("behind-cell-doors") if self.opened < BEHIND_CELL_DOORS_CELLS => {
                Some(Objective::BehindCellDoors {
                    opened: self.opened,
                })
            }
            Some("survival-of-the-fittest") if self.consumed < SURVIVAL_CONSUMES => {
                Some(Objective::SurvivalOfTheFittest {
                    consumed: self.consumed,
                })
            }
            _ => None,
        };
        // the warden's own cell has set its incident off already (or is the one being decided in)
        s.incidents = self.incidents & !(1u16 << warden) & !1;
        s.usurp = self.usurp;
        s.chain = self.chain_at.is_some();
        for &(i, r) in &self.revealed {
            if s.cells[i] == Cell::Locked {
                s.show(i, r);
            }
        }
        s
    }

    /// A Prison Incident set off (its card opened): counts for Undercurrents.
    pub fn incident_triggered(&mut self) {
        self.triggered += 1;
        if self.objective.as_deref() == Some("undercurrents")
            && self.triggered >= UNDERCURRENTS_INCIDENTS
        {
            self.objective = Some("done".into());
        }
    }

    /// What isn't known or played, said once at start and when it changes.
    pub fn gaps(&self) -> Vec<String> {
        let mut g = Vec::new();
        if self.rumor.is_none() {
            g.push(
                "Rumor: none set (type `rumor opulence`, `rumor cage-fight`, `rumor tyranny` or `rumor iron-law` if this run has one)"
                    .into(),
            );
        } else if let Some(r) = &self.rumor {
            let k = crate::instruments::slug(r);
            if k != "opulence" && k != "cage-fight" && k != "tyranny" {
                g.push(format!("Rumor {r}: not in the engine, advice ignores it"));
            }
        }
        match self.objective.as_deref() {
            None => g.push("objective: not set (type `objective prison-walk 0`, `objective king` or `objective done`)".into()),
            Some("prison-walk") | Some("king-of-the-caged") | Some("done") => {}
            Some("undercurrents") | Some("behind-cell-doors") | Some("survival-of-the-fittest") => {}
            Some(o) => g.push(format!("objective {o}: not in the engine, its instrument reward is ignored")),
        }
        for n in &self.instruments {
            match Instrument::find(n) {
                None => g.push(format!("instrument \"{n}\": unknown name, ignored")),
                Some(i) if !i.played() => {
                    g.push(format!("{}: not in the engine yet, ignored", i.name()))
                }
                _ => {}
            }
        }
        g
    }
}

fn parse_cell(t: &str) -> Cell {
    let mut p = t.split_whitespace();
    let (Some(c), Some(r)) = (p.next(), p.next()) else {
        return match t {
            "empty" => Cell::Empty,
            "office" => Cell::Office,
            _ => Cell::Locked,
        };
    };
    let code = match c {
        "elephant" => Code::Elephant,
        "lion" => Code::Lion,
        "wolf" => Code::Wolf,
        _ => Code::Rat,
    };
    let rarity = "NMRLUC".find(r).unwrap_or(0) as u8;
    Cell::Inmate(Inmate { code, rarity })
}

/// The advice for one picture.
#[derive(Clone, Debug, Default)]
pub struct Advice {
    /// What screen this was, for the console and the debug log.
    pub screen: String,
    /// One line on the board read.
    pub board: Option<String>,
    /// The options, best first: (description, expected Chromatics, margin).
    /// (description, expected Chromatics, margin, share of futures reaching 10).
    pub options: Vec<(String, f64, f64, f64)>,
    /// Where to draw: the best option's place on screen.
    pub boxes: Vec<Rect>,
    /// Things to tell the player: what was guessed, unread or not played.
    pub notes: Vec<String>,
    /// What the game did, for measuring its odds (JSON lines): doors opened, Replaces, destinations offered.
    pub observations: Vec<Value>,
    /// What the options' numbers are: "Chromatics", or "Ultimates" once the 10 Chromatics are in.
    pub unit: &'static str,
}

fn board_line(cells: &[Cell; CELLS], warden: usize, chromatics: u8) -> String {
    let mut s = Vec::new();
    for (n, c) in cells.iter().enumerate() {
        if let Cell::Inmate(m) = c {
            s.push(format!(
                "{}:{}{}",
                n + 1,
                &m.code.name()[..1],
                ["N", "M", "R", "L", "U", "C"][m.rarity as usize]
            ));
        }
    }
    format!(
        "at {} | {}/10 Chromatics | {}",
        warden + 1,
        chromatics,
        if s.is_empty() {
            "no Inmates".into()
        } else {
            s.join(" ")
        }
    )
}

fn run_solver(s: &State, phase: &Phase, futures: usize) -> Vec<solver::Outcome> {
    solver::evaluate_adaptive(s, phase, futures, 7)
}

/// The options as the console shows them: expected Chromatics, or, once the 10 are in, expected Ultimates (the red
/// beasts) left on the board at the end.
fn options(s: &State, outs: &[solver::Outcome]) -> Vec<(String, f64, f64, f64)> {
    let at10 = s.chromatics >= MAX_CHROMATICS;
    outs.iter()
        .map(|o| {
            if at10 {
                (o.action.describe(s), o.ultimates, o.ult_margin, o.full)
            } else {
                (o.action.describe(s), o.chromatics, o.margin, o.full)
            }
        })
        .collect()
}

/// Reads the picture and gives the advice. `popup` is what the log says is open; `state` is updated with what
/// the picture shows (the board, the warden, Prison Walk's tiles).
/// The advice on a picture, with the journal of the run (predictions, the box against what the player did, the end)
/// added to its observations.
pub fn advise(f: &Frame, popup: Popup, st: &mut RunState, futures: usize) -> Advice {
    let mut a = advise_screen(f, popup, st, futures);
    a.unit = if st.board.as_ref().is_some_and(|b| b.chromatics >= MAX_CHROMATICS) {
        "Ultimates"
    } else {
        "Chromatics"
    };
    journal(st, popup, &mut a);
    a
}

/// The warden's cell in a board line ("at 6 | ...").
fn line_warden(line: &str) -> Option<usize> {
    line.strip_prefix("at ")?
        .split_whitespace()
        .next()?
        .parse::<usize>()
        .ok()
        .map(|n| n - 1)
}

fn kind_of(label: &str) -> &'static str {
    let l = label.to_lowercase();
    if l.starts_with("go to") {
        "go"
    } else if l.starts_with("consume") {
        "consume"
    } else if l.starts_with("replace") {
        "replace"
    } else if l.starts_with("hunt") {
        "hunt"
    } else if l.starts_with("take") {
        "take"
    } else if l.starts_with("re-roll") {
        "reroll"
    } else {
        "end"
    }
}

fn journal(st: &mut RunState, popup: Popup, a: &mut Advice) {
    let day = st.day.map(|d| d.0);
    let chromatics = st.board.as_ref().map(|b| b.chromatics).unwrap_or(0);
    st.best_chromatics = st.best_chromatics.max(chromatics);
    if popup == Popup::End {
        if !st.ended {
            st.ended = true;
            a.observations.push(json!({"kind": "run_end", "chromatics": st.best_chromatics, "day": day,
                "instruments": st.instruments, "objective": st.objective, "rumor": st.rumor,
                "ultimates": st.board.as_ref().map(|b| b.cells.iter().filter(|c| matches!(c, Cell::Inmate(m) if m.rarity == ULTIMATE)).count())}));
        }
        st.pending = None;
        return;
    }
    let warden = a.board.as_deref().and_then(line_warden);
    let cells = st.board.as_ref().map(|b| b.cells);
    // what the player did on the last screen, now that this one shows it
    if let Some(p) = st.pending.clone() {
        let moved_on = a.screen != p.screen || warden != p.warden || cells != p.cells;
        let took: Option<String> = match p.screen.as_str() {
            "choose a destination" | "choose any cell" if warden.is_some() && warden != p.warden => warden.map(|w| {
                if w == 0 {
                    "go:office".to_string()
                } else {
                    format!("go:{}", w + 1)
                }
            }),
            "choose a Torture Instrument" if st.instruments.len() > p.instruments => {
                st.instruments.last().map(|n| format!("take:{n}"))
            }
            "make your decision" if a.screen == "choose what to Consume" => Some("consume".into()),
            "make your decision" if moved_on && warden.is_some() => match (p.cells, cells, p.warden) {
                (Some(old), Some(new), Some(w)) => {
                    let eaten = (0..CELLS).any(|n| {
                        n != w && matches!(old[n], Cell::Inmate(_)) && new[n] == Cell::Locked
                    });
                    Some(if eaten {
                        "consume".into()
                    } else if old[w] != new[w] {
                        "replace".into()
                    } else {
                        "end".into()
                    })
                }
                _ => None,
            },
            _ => None,
        };
        if let Some(t) = took {
            let boxed = match kind_of(&p.boxed) {
                "go" => {
                    if p.boxed.contains("Interrogation") {
                        "go:office".to_string()
                    } else {
                        p.boxed
                            .split_whitespace()
                            .nth(3)
                            .map(|n| format!("go:{n}"))
                            .unwrap_or_default()
                    }
                }
                "take" => format!("take:{}", p.boxed.trim_start_matches("Take ")),
                k => k.to_string(),
            };
            let value = |key: &str| {
                p.options
                    .iter()
                    .find(|o| {
                        let k = kind_of(&o.0);
                        match k {
                            "go" if key == "go:office" => o.0.contains("Interrogation"),
                            "go" => o.0.split_whitespace().nth(3).map(|n| format!("go:{n}")).as_deref() == Some(key),
                            "take" => format!("take:{}", o.0.trim_start_matches("Take ")) == key,
                            _ => k == key,
                        }
                    })
                    .map(|o| o.1)
            };
            a.observations.push(json!({"kind": "choice", "screen": p.screen, "day": p.day, "boxed": boxed,
                "took": t, "followed": boxed == t, "boxed_value": value(&boxed), "took_value": value(&t),
                "instruments": st.instruments, "objective": st.objective, "rumor": st.rumor}));
            st.pending = None;
        }
    }
    // this screen's prediction, once per screen
    let Some(best) = a.options.first().cloned() else { return };
    let key = format!("{}|{:?}|{:?}", a.screen, day, a.board);
    if key != st.journal_key {
        st.journal_key = key;
        a.observations.push(json!({"kind": "predict", "screen": a.screen, "day": day, "chromatics": chromatics,
            "expect": best.1, "margin": best.2, "reach10": best.3, "best": best.0}));
    }
    if matches!(
        a.screen.as_str(),
        "choose a destination" | "choose any cell" | "make your decision" | "choose a Torture Instrument"
    ) {
        st.pending = Some(Pending {
            screen: a.screen.clone(),
            boxed: best.0.clone(),
            options: a.options.iter().map(|o| (o.0.clone(), o.1)).collect(),
            day,
            warden,
            cells,
            instruments: st.instruments.len(),
        });
    }
}

fn advise_screen(f: &Frame, popup: Popup, st: &mut RunState, futures: usize) -> Advice {
    let mut a = Advice::default();
    if f.w != reader::WIDTH || f.h != reader::HEIGHT {
        a.screen = "unreadable".into();
        a.notes.push(format!(
            "the game picture is {}x{}: the reader needs 1920x1080 (borderless, minimum UI scale)",
            f.w, f.h
        ));
        return a;
    }
    match popup {
        Popup::Instrument | Popup::Treasure => {
            // "View Prison Status" hides the cards: read the board then, and advise with it
            let b = reader::read_board(f);
            if let (true, Some(warden)) = (b.on_board(), b.warden) {
                let mut notes = Vec::new();
                let cells = read_cells(&b, &mut notes);
                let picks = st.picks_done;
                remember(st, cells, warden, b.chromatics);
                st.picks_done = picks;
                st.board_fresh = true;
                let mut a = if popup == Popup::Treasure {
                    treasure(st, futures)
                } else {
                    Advice {
                        screen: "board (cards hidden)".into(),
                        ..Default::default()
                    }
                };
                a.screen = format!(
                    "{} (cards hidden)",
                    a.screen.trim_end_matches(" (cards hidden)")
                );
                a.board = Some(board_line(
                    &cells,
                    warden,
                    st.board.as_ref().map_or(b.chromatics, |x| x.chromatics),
                ));
                a.notes.retain(|n| !n.starts_with("the board behind"));
                a.notes.extend(notes);
                a.boxes.clear();
                if popup == Popup::Treasure {
                    a.notes
                        .push("board read: click the eye again to go back to the cards".into());
                    a.boxes.push(EYE);
                } else {
                    a.notes.push("board read with the cards hidden: show the cards again and the box follows".into());
                }
                return a;
            }
            return if popup == Popup::Instrument {
                instrument_offer(f, st, futures)
            } else {
                treasure(st, futures)
            };
        }
        Popup::Incident => {
            a.screen = "Prison Incident".into();
            // which card: Usurpation's next stage and a pending Chain Reaction matter for what comes
            if let Some(card) = reader::read_incident(f) {
                a.board = Some(format!("incident: {card}"));
                match card.as_str() {
                    "Usurpation I" => st.usurp = st.usurp.max(1),
                    "Usurpation II" => st.usurp = st.usurp.max(2),
                    "Usurpation III" => st.usurp = st.usurp.max(3),
                    "Usurpation IV" => st.usurp = 4,
                    "Chain Reaction" => st.chain_at = Some(st.day.map_or(0, |d| d.0)),
                    _ => {}
                }
            }
            a.notes.push(format!(
                "a Prison Incident: Confirm it (every one helps or does nothing). Incidents this run: {}{}",
                st.triggered,
                if st.objective.as_deref() == Some("undercurrents") { format!(" of {UNDERCURRENTS_INCIDENTS} for Undercurrents") } else { String::new() }
            ));
            return a;
        }
        Popup::Rumor => {
            a.screen = "Rumor card".into();
            match reader::read_rumor(f) {
                // the run's Rumor is drawn at its start; the card opened again mid-run (its icon) only confirms it
                Some(r) if st.rumor.is_none() || st.day.map_or(true, |d| d.0 == 0) => {
                    a.board = Some(format!("Rumor: {r}"));
                    st.rumor = Some(r);
                }
                Some(r) => {
                    a.board = Some(format!("Rumor: {r}"));
                    if st.rumor.as_deref() != Some(r.as_str()) {
                        a.notes.push(format!("this card reads as {r} but the run's Rumor is {}: type `rumor {r}` if that's wrong", st.rumor.as_deref().unwrap_or("none")));
                    }
                }
                None => a.notes.push("a Rumor I can't read yet: type `rumor opulence`, `rumor cage-fight`, `rumor tyranny` or `rumor iron-law` (tell me its name too)".into()),
            }
            return a;
        }
        Popup::Objective => {
            a.screen = "objective card".into();
            match reader::read_objective(f) {
                Some(o) => {
                    a.board = Some(format!("objective: {o}"));
                    // drawn at the run's start; opened again mid-run (its panel), it doesn't reset what's counted
                    let start = st.objective.is_none() || st.day.map_or(true, |d| d.0 == 0);
                    if start && st.objective.as_deref() != Some(o.as_str()) {
                        st.walked = 0;
                        st.objective = Some(o);
                    }
                }
                None => a.notes.push("I can't read this objective yet: type `objective prison-walk 0`, `objective king`, `objective undercurrents` or `objective <name>`".into()),
            }
            return a;
        }
        Popup::End => {
            a.screen = "end of the run".into();
            return a;
        }
        Popup::None => st.board_fresh = false,
    }
    let b = reader::read_board(f);
    if !b.on_board() {
        a.screen = "not the board".into();
        return a;
    }
    let Some(warden) = b.warden else {
        a.screen = "board (moving?)".into();
        a.notes
            .push("couldn't find the warden's arrow: waiting for the next picture".into());
        return a;
    };
    if matches!(
        b.label,
        Some(Label::Destination) | Some(Label::Decision) | Some(Label::AnyCell)
    ) {
        st.incidents = (0..CELLS)
            .filter(|&n| b.incidents[n])
            .fold(0u16, |m, n| m | 1 << n);
        st.revealed = (0..CELLS)
            .filter_map(|n| b.revealed[n].map(|r| (n, r)))
            .collect();
    }
    let mut a = match b.label {
        Some(Label::Destination) => destination(&b, warden, st, futures),
        Some(Label::Decision) => decision(f, &b, warden, st, futures),
        Some(Label::AnyCell) => any_cell(&b, warden, st, futures),
        None if b.red > 7.0 => consume_targets(f, &b, warden, st, futures),
        None => {
            a.screen = "board (between screens)".into();
            a
        }
    };
    if let Some(line) = a.board.as_mut() {
        let marks: Vec<String> = (1..CELLS)
            .filter(|&n| st.incidents & (1 << n) != 0)
            .map(|n| (n + 1).to_string())
            .collect();
        if !marks.is_empty() {
            line.push_str(&format!("  incidents (!): {}", marks.join(", ")));
        }
        if !st.revealed.is_empty() {
            let q: Vec<String> = st
                .revealed
                .iter()
                .map(|&(n, r)| {
                    format!("{}:{}", n + 1, ["N", "M", "R", "L", "U"][r.min(4) as usize])
                })
                .collect();
            line.push_str(&format!("  ? doors: {}", q.join(" ")));
        }
    }
    a
}

fn read_cells(b: &BoardRead, notes: &mut Vec<String>) -> [Cell; CELLS] {
    let (cells, n) = reader::to_cells(b);
    notes.extend(n);
    cells
}

fn remember(st: &mut RunState, cells: [Cell; CELLS], warden: usize, chromatics: u8) {
    // Behind Cell Doors: locked cells found open since the last board
    if let Some(old) = &st.board {
        st.opened += (0..CELLS)
            .filter(|&n| {
                old.cells[n] == Cell::Locked && matches!(cells[n], Cell::Inmate(_) | Cell::Empty)
            })
            .count() as u32;
    } else {
        st.opened += cells
            .iter()
            .filter(|c| matches!(c, Cell::Inmate(_) | Cell::Empty))
            .count()
            .saturating_sub(4) as u32;
    }
    if st.objective.as_deref() == Some("behind-cell-doors") && st.opened >= BEHIND_CELL_DOORS_CELLS
    {
        st.objective = Some("done".into());
    }
    // the Chromatics never go down in a run: a lower count is a misread
    let chromatics = st
        .board
        .as_ref()
        .map_or(chromatics, |b| chromatics.max(b.chromatics));
    st.board = Some(SavedBoard {
        cells,
        warden,
        chromatics,
    });
    st.picks_done = 0;
}

/// "Select an Objective" (Sewer Ratway): move to any cell but the Interrogation Room.
fn any_cell(b: &BoardRead, warden: usize, st: &mut RunState, futures: usize) -> Advice {
    let mut a = Advice {
        screen: "choose any cell".into(),
        ..Default::default()
    };
    let cells = read_cells(b, &mut a.notes);
    remember(st, cells, warden, b.chromatics);
    let s = st.state(
        cells,
        warden,
        st.board.as_ref().map_or(b.chromatics, |x| x.chromatics),
    );
    a.board = Some(board_line(
        &cells,
        warden,
        st.board.as_ref().map_or(b.chromatics, |x| x.chromatics),
    ));
    let dests: Vec<usize> = (1..CELLS).filter(|&c| c != warden).collect();
    let outs = run_solver(&s, &Phase::Move(dests), futures);
    a.options = options(&s, &outs);
    if let Some(Action::Go(c)) = outs.first().map(|o| o.action) {
        a.boxes.push(cell_box(c));
    }
    a
}

/// What changed since the last board read, as the game's odds see it: doors that opened (by the warden walking in,
/// or otherwise), a Replace (the warden's Inmate changed with nobody eaten), the destinations offered.
fn observe(
    st: &mut RunState,
    b: &BoardRead,
    cells: &[Cell; CELLS],
    warden: usize,
    screen: &'static str,
) -> Vec<Value> {
    let mut out = Vec::new();
    let day = st.day.map(|d| d.0);
    let held = st.instruments.clone();
    if let Some(old) = &st.board {
        let same_day = st.board_day == day;
        for n in 1..CELLS {
            if old.cells[n] == Cell::Locked {
                if let Cell::Inmate(m) = cells[n] {
                    let how = if n == warden && old.warden != warden {
                        "walk"
                    } else {
                        "other"
                    };
                    let was = st.revealed.iter().find(|r| r.0 == n).map(|r| r.1);
                    out.push(json!({"kind": "open", "cell": n + 1, "rarity": m.rarity, "code": m.code.name(), "how": how,
                        "shown": was, "day": day, "instruments": held, "rumor": st.rumor}));
                }
            }
        }
        // a Replace: decision to decision, same day and cell, the warden's Inmate changed and nobody else gone
        if screen == "decision"
            && st.board_screen == Some("decision")
            && same_day
            && old.warden == warden
        {
            let gone = (1..CELLS).any(|n| {
                n != warden
                    && matches!(old.cells[n], Cell::Inmate(_))
                    && !matches!(cells[n], Cell::Inmate(_))
            });
            if let (Cell::Inmate(o), Cell::Inmate(m), false) =
                (old.cells[warden], cells[warden], gone)
            {
                if o != m {
                    out.push(json!({"kind": "replace", "old": o.rarity, "old_code": o.code.name(), "new": m.rarity, "code": m.code.name(),
                        "day": day, "instruments": held, "rumor": st.rumor}));
                }
            }
        }
    }
    if screen == "destination" {
        let k = st.state(*cells, warden, 0).destination_count();
        let steps: Vec<usize> = b
            .destinations(k)
            .into_iter()
            .map(|d| (d + CELLS - warden) % CELLS)
            .collect();
        if st.board_screen != Some("destination")
            || st.board.as_ref().map(|o| o.warden) != Some(warden)
        {
            out.push(json!({"kind": "destinations", "at": warden + 1, "steps": steps, "count": k, "office_lit": b.office_lit, "day": day}));
        }
    }
    st.board_screen = Some(screen);
    st.board_day = day;
    out
}

fn destination(b: &BoardRead, warden: usize, st: &mut RunState, futures: usize) -> Advice {
    let mut a = Advice {
        screen: "choose a destination".into(),
        ..Default::default()
    };
    let cells = read_cells(b, &mut a.notes);
    a.observations = observe(st, b, &cells, warden, "destination");
    // Prison Walk: the tiles since the last destination screen
    if let (Some(last), Some("prison-walk")) = (st.last_warden, st.objective.as_deref()) {
        st.walked += ((warden + CELLS - last) % CELLS) as u32;
        if st.walked >= PRISON_WALK_TILES {
            st.objective = Some("done".into());
        }
    }
    st.last_warden = Some(warden);
    st.eater_rarity = None;
    // a Chain Reaction opens the cells on the next move (the next day's): gone the day after
    if let (Some(set), Some((today, _))) = (st.chain_at, st.day) {
        if today >= set + 2 {
            st.chain_at = None;
        }
    }
    remember(st, cells, warden, b.chromatics);
    let s = st.state(
        cells,
        warden,
        st.board.as_ref().map_or(b.chromatics, |x| x.chromatics),
    );
    let k = s.destination_count();
    let dests = b.destinations(k);
    a.board = Some(board_line(
        &cells,
        warden,
        st.board.as_ref().map_or(b.chromatics, |x| x.chromatics),
    ));
    if dests.is_empty() {
        a.notes
            .push("couldn't see which destinations are offered".into());
        return a;
    }
    if dests.len() < k {
        a.notes.push(format!(
            "saw only {} of the {} destinations offered",
            dests.len(),
            k
        ));
    }
    let outs = run_solver(&s, &Phase::Move(dests.clone()), futures);
    a.options = options(&s, &outs);
    if let Some(Action::Go(c)) = outs.first().map(|o| o.action) {
        let pos = dests.iter().position(|&d| d == c).unwrap_or(0);
        a.boxes.push(bottom(dests.len(), pos));
        a.boxes.push(if c == OFFICE {
            r(462, 82, 580, 192)
        } else {
            cell_box(c)
        });
    }
    a
}

/// How many buttons the decision screen shows (20: the two are Consume and End): 4 when the place left of the
/// three-button row (x 300-430) is textured, 3 when the leftmost card's place (x 470-600) is textured, 2 when only
/// the middle-left place (x 640-780) is, None when it can't tell (screen fading).
fn button_count(f: &Frame) -> Option<usize> {
    let tex = |x0: usize, x1: usize| -> f64 {
        let mut sum = 0.0;
        let mut n = 0.0;
        for y in (770..900).step_by(2) {
            for x in x0..x1 - 1 {
                let i = (y * f.w + x) * 3;
                let j = i + 3;
                let g = |k: usize| {
                    f.rgb[k] as f64 * 0.299
                        + f.rgb[k + 1] as f64 * 0.587
                        + f.rgb[k + 2] as f64 * 0.114
                };
                sum += (g(j) - g(i)).abs();
                n += 1.0;
            }
        }
        sum / n
    };
    if f.w < 1920 || f.h < 1080 {
        return None;
    }
    // the red of the Consume button in the two-button place (Consume and End, once Replace is used up)
    let mut red = 0.0;
    let mut n = 0.0;
    for y in (770..900).step_by(2) {
        for x in (650..930).step_by(2) {
            let p = f.px(x, y);
            red += (p[0] - p[1]) as f64;
            n += 1.0;
        }
    }
    let red = red / n;
    if tex(300, 430) > 2.0 {
        Some(4) // Consume, Replace, Hunt, End (Blood-Stained Trap on a Lion)
    } else if tex(470, 600) > 2.0 {
        Some(3)
    } else if red > 15.0 {
        Some(20) // two buttons: Consume and End
    } else if tex(640, 780) > 2.0 {
        Some(2)
    } else {
        None
    }
}

fn decision(f: &Frame, b: &BoardRead, warden: usize, st: &mut RunState, futures: usize) -> Advice {
    let mut a = Advice::default();
    let cells = read_cells(b, &mut a.notes);
    a.observations = observe(st, b, &cells, warden, "decision");
    remember(st, cells, warden, b.chromatics);
    a.board = Some(board_line(
        &cells,
        warden,
        st.board.as_ref().map_or(b.chromatics, |x| x.chromatics),
    ));
    let s = st.state(
        cells,
        warden,
        st.board.as_ref().map_or(b.chromatics, |x| x.chromatics),
    );
    if warden == OFFICE {
        a.screen = "Interrogation Room".into();
        let outs = run_solver(&s, &Phase::Decide, futures);
        a.options = options(&s, &outs);
        if let Some(o) = outs.first() {
            // the two buttons' places are a guess (no 1080p screenshot of this screen yet)
            a.boxes
                .push(bottom(2, if o.action == Action::Bestow { 1 } else { 0 }));
        }
        return a;
    }
    a.screen = "make your decision".into();
    st.eater_rarity = s.inmate(warden).map(|m| m.rarity);
    let mut outs = run_solver(&s, &Phase::Decide, futures);
    a.options = options(&s, &outs);
    let acts = solver::actions(&s, &Phase::Decide);
    let has_consume = acts.iter().any(|x| matches!(x, Action::Consume(_)));
    let has_hunt = acts.iter().any(|x| matches!(x, Action::Hunt(_)));
    let kind = |x: &Action| match x {
        Action::Consume(_) => "consume",
        Action::Hunt(_) => "hunt",
        Action::Replace => "replace",
        _ => "end",
    };
    // the buttons as the game shows them: read from the picture, the solver's view only as a fallback
    let buttons: Vec<&str> = match button_count(f) {
        // the game's order is Consume, Replace, Hunt, End, without the ones not offered (measured on 1080p
        // pictures: all four, and Consume, Hunt, End once the day's Replace was used)
        Some(4) => vec!["consume", "replace", "hunt", "end"],
        Some(3) if has_hunt && has_consume => vec!["consume", "hunt", "end"],
        Some(3) if has_hunt => vec!["replace", "hunt", "end"],
        Some(3) => vec!["consume", "replace", "end"],
        Some(20) if has_hunt && !has_consume => vec!["hunt", "end"],
        Some(20) => vec!["consume", "end"],
        Some(2) => vec!["replace", "end"],
        _ => {
            let mut v = Vec::new();
            if has_consume {
                v.push("consume");
            }
            if s.bites_today == 0 {
                v.push("replace");
            }
            if has_hunt {
                v.push("hunt");
            }
            v.push("end");
            v
        }
    };
    if !has_consume && buttons.contains(&"consume") {
        a.notes.push(
            "the game offers Consume but the board as read has nothing to eat: a cell is misread"
                .into(),
        );
    }
    if has_consume && !buttons.contains(&"consume") && !buttons.contains(&"hunt") {
        a.notes.push("the game offers no Consume though the board as read has prey: a cell is misread, advice is among what's on screen".into());
    }
    // only what the game offers now: after a Replace the Replace button is gone (and the box mustn't stay on it)
    if button_count(f).is_some() {
        outs.retain(|o| buttons.contains(&kind(&o.action)));
        a.options = options(&s, &outs);
    }
    // the best option among the buttons on screen
    if let Some(o) = outs.iter().find(|o| buttons.contains(&kind(&o.action))) {
        let k = buttons
            .iter()
            .position(|b| *b == kind(&o.action))
            .unwrap_or(0);
        a.boxes.push(bottom(buttons.len(), k));
        if let Action::Consume(t) | Action::Hunt(t) = o.action {
            a.boxes.push(cell_box(t));
        }
        if outs.first().map(|f| f.action != o.action).unwrap_or(false) {
            a.notes.push(format!(
                "boxed: {} (best among the buttons on screen)",
                kind(&o.action)
            ));
        }
    }
    a
}

fn consume_targets(
    f: &Frame,
    b: &BoardRead,
    warden: usize,
    st: &mut RunState,
    futures: usize,
) -> Advice {
    let mut a = Advice {
        screen: "choose what to Consume".into(),
        ..Default::default()
    };
    // Under the red, only the eater and its targets show, tinted (rarities can't be read): the rest of the board is
    // the last one read, and the targets are the cells with jaws.
    let Some(saved) = st.board.clone() else {
        a.notes.push(
            "no board read before this screen: open the board once (Cancel) so I can see it".into(),
        );
        return a;
    };
    let jaws: Vec<usize> = {
        let j = reader::read_jaws(f);
        (1..CELLS).filter(|&n| j[n] && n != warden).collect()
    };
    let mut cells = saved.cells;
    // the board shows through the red: a cell found locked again was eaten earlier in the chain
    for n in 1..CELLS {
        if n != warden && matches!(cells[n], Cell::Inmate(_)) && b.cells[n] == CellRead::Locked {
            cells[n] = Cell::Locked;
        }
    }
    let saved_eater = saved.cells[warden];
    let mut eater = match (b.cells[warden], saved_eater) {
        (CellRead::Inmate { code, rarity, .. }, Cell::Inmate(m)) => Inmate {
            code,
            rarity: rarity.min(m.rarity.max(rarity)),
        },
        (CellRead::Inmate { code, rarity, .. }, _) => Inmate { code, rarity },
        (_, Cell::Inmate(m)) => m,
        _ => {
            a.notes.push("can't see the eater".into());
            return a;
        }
    };
    // the eater's rarity: the one (from its last known up) whose targets are exactly the jaws
    if !jaws.is_empty() {
        let low = match saved_eater {
            Cell::Inmate(m) => m.rarity,
            _ => NORMAL,
        };
        // a cell with jaws not on the last board (opened since): its Code from the screen, its rarity the eater's
        let fresh: Vec<(usize, Code)> = jaws
            .iter()
            .filter(|&&n| !matches!(cells[n], Cell::Inmate(_)))
            .filter_map(|&n| match b.cells[n] {
                CellRead::Inmate { code, .. } => Some((n, code)),
                _ => None,
            })
            .collect();
        if !fresh.is_empty() {
            a.notes.push(format!(
                "cells {:?} opened since the last board: their rarity is a guess",
                fresh.iter().map(|f| f.0 + 1).collect::<Vec<_>>()
            ));
        }
        for &(n, code) in &fresh {
            cells[n] = Cell::Inmate(Inmate {
                code,
                rarity: eater.rarity,
            });
        }
        // the Code read under the red first, then the others (a Consume changes the eater's Code); then, if
        // nothing fits, the jaws' cells as new too (Avenger's Casket refills an eaten cell with a meal at the
        // eater's new rarity)
        let mut codes = vec![eater.code];
        codes.extend(Code::ALL.into_iter().filter(|&c| c != eater.code));
        let all_jaws: Vec<(usize, Code)> = jaws
            .iter()
            .filter_map(|&n| match (b.cells[n], cells[n]) {
                (CellRead::Inmate { code, .. }, _) => Some((n, code)),
                (_, Cell::Inmate(m)) => Some((n, m.code)),
                _ => None,
            })
            .collect();
        'search: for set in [&fresh, &all_jaws] {
            for &code in &codes {
                for r in low..ULTIMATE + 1 {
                    let mut c = cells;
                    for &(n, fc) in set.iter() {
                        c[n] = Cell::Inmate(Inmate {
                            code: fc,
                            rarity: r,
                        });
                    }
                    c[warden] = Cell::Inmate(Inmate { code, rarity: r });
                    let mut t = st.state(c, warden, saved.chromatics).targets(warden);
                    t.sort();
                    if t == jaws {
                        eater = Inmate { code, rarity: r };
                        cells = c;
                        break 'search;
                    }
                }
            }
        }
    }
    cells[warden] = Cell::Inmate(eater);
    // keep this board for what comes next (a Warden's Treasure covers the board)
    st.board = Some(SavedBoard {
        cells,
        warden,
        chromatics: saved.chromatics,
    });
    let mut s = st.state(cells, warden, saved.chromatics);
    s.bites_today = match st.eater_rarity {
        Some(before) if eater.rarity > before => (eater.rarity - before) as u32,
        _ => 0,
    };
    a.board = Some(board_line(&cells, warden, saved.chromatics));
    let mut outs = run_solver(&s, &Phase::Decide, futures);
    outs.retain(|o| match o.action {
        Action::Consume(t) => jaws.is_empty() || jaws.contains(&t),
        Action::End => true,
        _ => false,
    });
    a.options = options(&s, &outs);
    if !jaws.is_empty() {
        let mut t = s.targets(warden);
        t.sort();
        if t != jaws {
            a.notes.push(format!(
                "jaws on {:?} but the board as read gives {:?}: a cell is misread",
                jaws.iter().map(|n| n + 1).collect::<Vec<_>>(),
                t.iter().map(|n| n + 1).collect::<Vec<_>>()
            ));
        }
    }
    let best_consume = outs.iter().any(|o| matches!(o.action, Action::Consume(_)));
    match outs.first().map(|o| o.action) {
        Some(Action::Consume(t)) => a.boxes.push(cell_box(t)),
        Some(_) if best_consume || jaws.is_empty() => {
            a.boxes.push(CANCEL);
            if let Some(o) = a.options.first_mut() {
                o.0 = "Stop eating (Cancel, then End)".into();
            }
        }
        Some(_) | None if !jaws.is_empty() => {
            // the solver can't play these targets (misread board): the jaws are what the game allows
            a.boxes.push(cell_box(jaws[0]));
            a.notes
                .push("boxed the first cell with jaws: the solver couldn't weigh them".into());
        }
        _ => a
            .notes
            .push("no target I can see: the board under the red may be misread".into()),
    }
    a
}

/// The Re-roll button's "Remaining attempts" line is bright when there's a re-roll left.
fn reroll_visible(f: &Frame) -> bool {
    let mut s = 0.0;
    for y in 898..916 {
        for x in 700..900 {
            s += f.gray(x, y);
        }
    }
    s / (18.0 * 200.0) > 35.0
}

fn instrument_offer(f: &Frame, st: &mut RunState, futures: usize) -> Advice {
    let mut a = Advice {
        screen: "choose a Torture Instrument".into(),
        ..Default::default()
    };
    let mut read = reader::read_instrument_offer(f);
    // same offer as the last picture (every card read agrees), one card missed: keep the name it had
    if read.len() == st.last_offer.len()
        && read.iter().any(|r| r.is_some())
        && read
            .iter()
            .zip(&st.last_offer)
            .all(|(r, l)| r.as_ref().map_or(true, |r| l.as_deref() == Some(r.0.as_str())))
    {
        for (r, l) in read.iter_mut().zip(&st.last_offer) {
            if r.is_none() {
                *r = l.clone().map(|n| (n, 0.0));
            }
        }
    }
    if read.iter().any(|r| r.is_some()) {
        st.last_offer = read.iter().map(|r| r.as_ref().map(|r| r.0.clone())).collect();
    }
    if let Some(k) = reader::selected_instrument_slot(f) {
        st.selected_instrument = read.get(k).cloned().flatten().map(|x| x.0);
        st.selected_guess = false;
    }
    if read.iter().all(|r| r.is_none()) {
        // the cards are still sliding in: the next picture will do
        a.screen = "instrument offer (cards moving)".into();
        return a;
    }
    let mut offers = Vec::new();
    for (k, r) in read.iter().enumerate() {
        match r {
            Some((name, _)) => match Instrument::find(name) {
                Some(i) => offers.push((k, i)),
                None => a
                    .notes
                    .push(format!("card {}: {name} isn't in the engine", k + 1)),
            },
            None => a.notes.push(format!(
                "card {}: I can't read this instrument's name yet (tell me what it is)",
                k + 1
            )),
        }
    }
    a.board = Some(
        offers
            .iter()
            .map(|(_, i)| i.name())
            .collect::<Vec<_>>()
            .join(" / "),
    );
    if offers.is_empty() {
        return a;
    }
    let (cells, warden, chroma) = match &st.board {
        Some(b) => (b.cells, b.warden, b.chromatics),
        None => {
            let mut c = [Cell::Locked; CELLS];
            c[OFFICE] = Cell::Office;
            (c, OFFICE, 0)
        }
    };
    let mut s = st.state(cells, warden, chroma);
    s.rerolls = reroll_visible(f) as u32;
    let phase = Phase::Instrument(offers.iter().map(|x| x.1).collect());
    let outs = run_solver(&s, &phase, futures);
    a.options = options(&s, &outs);
    match outs.first().map(|o| o.action) {
        Some(Action::Take(i)) => {
            if let Some((k, _)) = offers.iter().find(|x| x.1 == i) {
                a.boxes.push(INSTRUMENT_CARDS[*k]);
            }
        }
        Some(Action::Reroll) => a.boxes.push(REROLL),
        _ => {}
    }
    // no selected card seen yet: if Confirm comes before one is, the boxed card is taken as the one taken
    if st.selected_instrument.is_none() || st.selected_guess {
        if let Some(Action::Take(i)) = outs
            .iter()
            .map(|o| o.action)
            .find(|x| matches!(x, Action::Take(_)))
        {
            st.selected_instrument = Some(i.name().to_string());
            st.selected_guess = true;
        }
    }
    if offers.len() < 3 {
        a.notes.push(
            "not every card was read, so the advice only compares the ones I could read".into(),
        );
    }
    a
}

fn treasure(st: &mut RunState, futures: usize) -> Advice {
    let mut a = Advice {
        screen: "choose a Warden's Treasure".into(),
        ..Default::default()
    };
    let Some(b) = st.board.clone() else {
        a.notes
            .push("no board read yet, so no advice on the treasure".into());
        return a;
    };
    let mut cells = b.cells;
    let mut chromatics = b.chromatics;
    // read during the Consume chain: the eater that just became a Chromatic is still in its cell on that board
    if !st.board_fresh && st.eater_rarity.is_some() {
        if let Cell::Inmate(m) = cells[b.warden] {
            if m.rarity >= LEGENDARY {
                cells[b.warden] = Cell::Locked;
                chromatics = (chromatics + 1).min(MAX_CHROMATICS);
            }
        }
    }
    let mut s = st.state(cells, b.warden, chromatics);
    let per: u32 = if s.setup.opulence { 3 } else { 1 };
    s.picks = per.saturating_sub(st.picks_done).max(1);
    let outs: Vec<solver::Outcome> = if s.picks >= 2 {
        // several picks to come (Opulence): judge each first pick with its best second
        let pairs = solver::evaluate_treasure_pairs(&s, futures, 7);
        a.options = pairs
            .iter()
            .map(|(o, t2)| {
                (
                    format!("{} (then {})", o.action.describe(&s), t2.name()),
                    o.chromatics,
                    o.margin,
                    o.full,
                )
            })
            .collect();
        pairs.into_iter().map(|p| p.0).collect()
    } else {
        let outs = run_solver(&s, &Phase::Treasure, futures);
        a.options = options(&s, &outs);
        outs
    };
    if let Some(Action::Pick(t)) = outs.first().map(|o| o.action) {
        let k = Treasure::ALL.iter().position(|x| *x == t).unwrap_or(0);
        a.boxes.push(TREASURE_CARDS[k]);
    }
    if !st.board_fresh {
        // look at the board first: the eye hides the cards, and the advice follows on the board as it is now
        a.notes.push(
            "click the eye (top right) so I can see the board, then click it again for the cards"
                .into(),
        );
        a.boxes.clear();
        a.boxes.push(EYE);
    }
    a
}
