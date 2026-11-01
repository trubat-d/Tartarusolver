//! Reading a board from JSON (see `boards/` for examples).

use crate::game::*;
use crate::instruments::Instrument;
use crate::solver::Phase;
use serde_json::Value;

pub struct Board {
    pub state: State,
    pub phase: Phase,
    /// Instruments, Rumors or cells the engine doesn't play yet.
    pub warnings: Vec<String>,
}

fn code_of(name: &str) -> Option<Code> {
    match name.to_ascii_lowercase().as_str() {
        "elephant" | "mammoth" => Some(Code::Elephant),
        "lion" => Some(Code::Lion),
        "wolf" => Some(Code::Wolf),
        "rat" => Some(Code::Rat),
        _ => None,
    }
}

fn rarity_of(name: &str) -> Option<u8> {
    match name.to_ascii_lowercase().as_str() {
        "n" | "normal" | "white" => Some(NORMAL),
        "m" | "magic" | "blue" => Some(MAGIC),
        "r" | "rare" | "purple" => Some(RARE),
        "l" | "legendary" | "orange" => Some(LEGENDARY),
        "u" | "ultimate" | "red" | "supreme" => Some(ULTIMATE),
        _ => None,
    }
}

/// "locked", "empty", "office", or a Code and a rarity: "wolf L", "lion magic".
fn cell_of(text: &str) -> Result<Cell, String> {
    let t = text.trim().to_ascii_lowercase();
    match t.as_str() {
        "locked" | "" => return Ok(Cell::Locked),
        "empty" => return Ok(Cell::Empty),
        "office" | "interrogation" | "interrogation room" => return Ok(Cell::Office),
        _ => {}
    }
    let mut parts = t.split_whitespace();
    let (Some(c), Some(r), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err(format!(
            "\"{text}\": expected a Code and a rarity, like \"wolf L\""
        ));
    };
    let code = code_of(c).ok_or(format!("\"{text}\": unknown Code {c}"))?;
    let rarity = rarity_of(r).ok_or(format!("\"{text}\": unknown rarity {r} (N, M, R, L or U)"))?;
    Ok(Cell::Inmate(Inmate { code, rarity }))
}

fn cell_index(v: &Value, what: &str) -> Result<usize, String> {
    let n = v
        .as_u64()
        .ok_or(format!("{what}: expected a cell number"))? as usize;
    if !(1..=CELLS).contains(&n) {
        return Err(format!("{what}: cell {n} is not between 1 and {CELLS}"));
    }
    Ok(n - 1)
}

pub fn parse(text: &str) -> Result<Board, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
    let mut warnings = Vec::new();
    let mut setup = Setup::default();
    match v["rumor"].as_str() {
        None => {}
        Some(r) if r.eq_ignore_ascii_case("opulence") => setup.opulence = true,
        Some(r) if crate::instruments::slug(r) == "cage-fight" => setup.cage_fight = true,
        Some(r) if crate::instruments::slug(r) == "tyranny" => setup.tyranny = true,
        Some(r) => warnings.push(format!(
            "the Rumor {r} isn't in the engine yet: advice ignores it"
        )),
    }
    for i in v["instruments"].as_array().into_iter().flatten() {
        let name = i.as_str().unwrap_or_default();
        match Instrument::find(name) {
            Some(ins) => {
                setup.instruments.add(ins);
                if !ins.played() {
                    warnings.push(format!(
                        "{} isn't in the engine yet: advice ignores it",
                        ins.name()
                    ));
                }
            }
            None => warnings.push(format!("unknown instrument \"{name}\": advice ignores it")),
        }
    }
    let mut cells = [Cell::Locked; CELLS];
    cells[OFFICE] = Cell::Office;
    if let Some(map) = v["cells"].as_object() {
        for (k, c) in map {
            let n: usize = k
                .parse()
                .map_err(|_| format!("cells: \"{k}\" is not a cell number"))?;
            if !(1..=CELLS).contains(&n) {
                return Err(format!("cells: cell {n} is not between 1 and {CELLS}"));
            }
            cells[n - 1] = cell_of(c.as_str().ok_or(format!("cells.{k}: expected text"))?)?;
        }
    }
    let days = v["days_until_riot"]
        .as_u64()
        .ok_or("days_until_riot: expected the number shown at the top")?;
    let pos = cell_index(&v["at"], "at")?;
    let mut state = State::new(cells, pos, days as u32 + 1, setup); // the screen shows the days after this one
    state.chromatics = v["chromatics"]
        .as_u64()
        .unwrap_or(0)
        .min(MAX_CHROMATICS as u64) as u8;
    state.picks = v["picks"].as_u64().unwrap_or(0) as u32;
    state.rerolls = v["rerolls"].as_u64().unwrap_or(0).min(1) as u32;
    state.bites_today = v["consumed_today"].as_u64().unwrap_or(0) as u32;
    if !v["hunt"].is_null() {
        let h = &v["hunt"];
        state.hunt = Some(Hunt {
            lion: cell_index(&h["lion"], "hunt.lion")?,
            prey: cell_index(&h["prey"], "hunt.prey")?,
            mornings: h["mornings"].as_u64().unwrap_or(HUNT_MORNINGS as u64) as u8,
        });
    }
    state.objective = match v["objective"].as_str().map(|o| crate::instruments::slug(o)) {
        None => None,
        Some(o) if o == "prison-walk" => Some(Objective::PrisonWalk {
            walked: v["walked"].as_u64().unwrap_or(0) as u32,
        }),
        Some(o) if o == "king-of-the-caged" => Some(Objective::KingOfTheCaged),
        Some(o) if o == "undercurrents" => Some(Objective::Undercurrents {
            triggered: v["triggered"].as_u64().unwrap_or(0) as u32,
        }),
        Some(o) if o == "behind-cell-doors" => Some(Objective::BehindCellDoors {
            opened: v["opened"].as_u64().unwrap_or(0) as u32,
        }),
        Some(o) if o == "survival-of-the-fittest" => Some(Objective::SurvivalOfTheFittest {
            consumed: v["consumed"].as_u64().unwrap_or(0) as u32,
        }),
        Some(o) if o == "done" => None,
        Some(o) => {
            warnings.push(format!(
                "the objective {o} isn't in the engine yet: advice ignores it"
            ));
            None
        }
    };
    for t in v["incidents"].as_array().into_iter().flatten() {
        let i = cell_index(t, "incidents")?;
        state.incidents |= 1 << i;
    }
    for t in v["tusks"].as_array().into_iter().flatten() {
        state.tusks.push(cell_index(t, "tusks")?);
    }
    // a scratch shows only the rarity: the Code behind it is drawn
    let mut rng = crate::rng::Rng::new(0x5C2A7C4);
    for (k, r) in v["scratches"].as_object().into_iter().flatten() {
        let n: usize = k
            .parse()
            .ok()
            .filter(|n| (1..=CELLS).contains(n))
            .ok_or(format!("scratches: \"{k}\" is not a cell number"))?;
        let rarity = rarity_of(r.as_str().unwrap_or_default())
            .ok_or(format!("scratches.{k}: a rarity (N, M, R, L or U)"))?;
        state.scratches.push((
            n - 1,
            Inmate {
                code: rng.pick(&Code::ALL),
                rarity,
            },
        ));
    }
    let phase = match v["choose"].as_str().unwrap_or("") {
        "destination" => {
            let options = v["options"]
                .as_array()
                .ok_or("options: the destinations offered, as cell numbers")?;
            Phase::Move(
                options
                    .iter()
                    .map(|o| cell_index(o, "options"))
                    .collect::<Result<_, _>>()?,
            )
        }
        "decision" => Phase::Decide,
        "treasure" => Phase::Treasure,
        "instrument" => {
            let offers = v["offers"]
                .as_array()
                .ok_or("offers: the Torture Instruments offered, by name")?;
            let mut found = Vec::new();
            for o in offers {
                let name = o.as_str().unwrap_or_default();
                found.push(
                    Instrument::find(name)
                        .ok_or(format!("offers: unknown instrument \"{name}\""))?,
                );
            }
            Phase::Instrument(found)
        }
        other => {
            return Err(format!(
                "choose: \"{other}\" should be destination, decision, treasure or instrument"
            ))
        }
    };
    if matches!(phase, Phase::Treasure) && state.picks == 0 {
        return Err("picks: how many treasure picks are left, this one included".into());
    }
    Ok(Board {
        state,
        phase,
        warnings,
    })
}
