//! The Torture Instruments (RULES.md, "All Torture Instruments"), by their effects in the engine.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Instrument {
    /// Lions eat any Inmate of their rarity; a Lion's Consume gains 3 tiers.
    IronCrown,
    /// On a Lion, Hunt: a Prey mark on a target; both go up a tier each morning for 3 mornings.
    BloodStainedTrap,
    /// A Scratch Mark each morning on a locked cell, showing its rarity; a Wolf of that rarity eats behind it for 3 tiers.
    FilthyClaws,
    /// Elephants also eat higher-rarity Lions and Wolves; an Elephant that eats rises to 2 tiers above its meal.
    ElephantFootMaul,
    /// A Replace comes in 3 tiers up when as many Inmates are above the replaced rarity as below.
    SeveredIronMaiden,
    /// Each morning, 2+ Inmates sharing Code and rarity: 60% that one is removed and another gains 2 tiers.
    KinsCoinOfFate,
    /// A Replace with 2+ others present removes 2 random Inmates; the new one comes in above the old rarity.
    ScapegoatsGuillotine,
    /// A Replace never brings Normal or Magic.
    HangmansNoose,
    /// A move of 4+ cells: 40% to raise a random Inmate a tier.
    LongChainedShackles,
    /// With 5 or fewer cells locked, each patrol opening raises 2 random Inmates a tier.
    ExecutionersBlacklist,
    /// A Consume gains 2 extra tiers with 4 or fewer Inmates on the board, its meal included.
    AgonizingMuzzle,
    /// After a Consume, 30% to open a locked cell holding a fresh meal at the eater's new rarity.
    AvengersCasket,
    /// Each morning, the highest-rarity Inmate removes both ring neighbours; 40% per removal for an extra tier.
    RavenousChains,
    /// Rats eat Rats of their rarity; a Rat's Consume gains 2 extra tiers.
    SkeweredRatHeads,
    /// Doors never hold a Normal.
    SortingHook,
    /// Each Consume after the first in a day: 80% for an extra tier.
    BarbedCollar,
    /// Each Consume: 25% for an extra tier.
    RustySaw,
    /// One destination fewer; replacing an Inmate below Legendary: 40% to bring a Legendary.
    BurningStake,
    /// Wolves also eat Rats below their rarity.
    FangedGuillotine,
    /// One destination fewer; each Consume: 35% for an extra tier.
    BarbedWhip,
    /// Each patrol-opened Inmate: 25% to come in a tier higher.
    FelonsDogTag,
    /// Each morning, an Elephant gets a Tusk mark; replacing it raises 4 other Inmates a tier.
    IronTongsOfLust,
    /// After a Consume, 40% that the eaten cell reopens at once with someone new.
    CrowdedBeastcage,
    /// 80% after a patrol opening to open both neighbours too.
    LostRingOfKeys,
    /// After a Replace, shows the best Inmate behind the locked doors: no effect on play.
    WardensScales,
    /// A move of 3 or fewer cells: 50% to open a random locked cell (setting nothing off).
    ShortChainedShackles,
    /// A Replace keeps the Code and re-rolls only the rarity.
    IronPestle,
    /// A patrol opening: 35% to copy its Inmate into another locked cell, which opens holding the copy.
    SearingBrandingIron,
    /// When a Replace raises the rarity, 2 random locked cells open.
    GaolersExecutionAxe,
    /// One destination fewer; 50% after a patrol opening to open 1 more random cell.
    ForgedKey,
    /// Each morning, turns a locked cell into a Rat nest, which hatches a Rare Rat 2 mornings later.
    SquirmingIronBox,
}

use Instrument::*;

pub const ALL: [Instrument; 31] = [
    IronCrown,
    BloodStainedTrap,
    FilthyClaws,
    ElephantFootMaul,
    SeveredIronMaiden,
    KinsCoinOfFate,
    ScapegoatsGuillotine,
    HangmansNoose,
    LongChainedShackles,
    ExecutionersBlacklist,
    AgonizingMuzzle,
    AvengersCasket,
    RavenousChains,
    SkeweredRatHeads,
    SortingHook,
    BarbedCollar,
    RustySaw,
    BurningStake,
    FangedGuillotine,
    BarbedWhip,
    FelonsDogTag,
    IronTongsOfLust,
    CrowdedBeastcage,
    LostRingOfKeys,
    WardensScales,
    ShortChainedShackles,
    IronPestle,
    SearingBrandingIron,
    GaolersExecutionAxe,
    ForgedKey,
    SquirmingIronBox,
];

impl Instrument {
    pub fn name(self) -> &'static str {
        match self {
            IronCrown => "Iron Crown of the Prison King",
            BloodStainedTrap => "Blood-Stained Trap",
            FilthyClaws => "Filthy Claws",
            ElephantFootMaul => "Elephant-Foot Maul",
            SeveredIronMaiden => "Severed Iron Maiden",
            KinsCoinOfFate => "Kin's Coin of Fate",
            ScapegoatsGuillotine => "Scapegoat's Guillotine",
            HangmansNoose => "Hangman's Noose",
            LongChainedShackles => "Long-Chained Shackles",
            ExecutionersBlacklist => "Executioner's Blacklist",
            AgonizingMuzzle => "Raging Beast's Agonizing Muzzle",
            AvengersCasket => "Avenger's Casket",
            RavenousChains => "Ferocious Beast's Ravenous Chains",
            SkeweredRatHeads => "Skewered Rat Heads",
            SortingHook => "Sorting Hook",
            BarbedCollar => "Maddened Beast's Barbed Collar",
            RustySaw => "Rusty Saw",
            BurningStake => "Burning Stake",
            FangedGuillotine => "Fanged Guillotine",
            BarbedWhip => "Barbed Whip",
            FelonsDogTag => "Felon's Dog Tag",
            IronTongsOfLust => "Iron Tongs of Lust",
            CrowdedBeastcage => "Crowded Beastcage",
            LostRingOfKeys => "Lost Ring of Keys",
            WardensScales => "Warden's Scales",
            ShortChainedShackles => "Short-Chained Shackles",
            IronPestle => "Iron Pestle",
            SearingBrandingIron => "Searing Branding Iron",
            GaolersExecutionAxe => "Gaoler's Execution Axe",
            ForgedKey => "Forged Key",
            SquirmingIronBox => "Squirming Iron Box",
        }
    }

    /// The name as a board file writes it: lowercase words joined by dashes, without apostrophes.
    pub fn key(self) -> String {
        slug(self.name())
    }

    /// The eight named after a Code, offered about 1/8 as often as the others.
    pub fn code_named(self) -> bool {
        matches!(
            self,
            IronCrown
                | BloodStainedTrap
                | FilthyClaws
                | ElephantFootMaul
                | SkeweredRatHeads
                | SquirmingIronBox
                | IronTongsOfLust
                | FangedGuillotine
        )
    }

    /// Whether the engine plays it (the others are ignored, with a note).
    pub fn played(self) -> bool {
        true
    }

    /// Finds an instrument by name, full or short ("Ravenous Chains", "ferocious-beasts-ravenous-chains",
    /// "Iron Crown of the Prison King: Berserk Lion").
    pub fn find(name: &str) -> Option<Instrument> {
        let s = slug(name.split(':').next().unwrap_or(name));
        ALL.into_iter().find(|i| {
            let k = i.key();
            k == s || k.ends_with(&format!("-{s}")) || k.starts_with(&format!("{s}-"))
        })
    }
}

pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for w in s.split(|c: char| !c.is_ascii_alphanumeric() && c != '\'') {
        let w: String = w
            .chars()
            .filter(|c| *c != '\'')
            .collect::<String>()
            .to_ascii_lowercase();
        if !w.is_empty() {
            if !out.is_empty() {
                out.push('-');
            }
            out.push_str(&w);
        }
    }
    out
}

/// The instruments a run holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Held(u32);

impl Held {
    pub fn add(&mut self, i: Instrument) {
        self.0 |= 1 << i as u32;
    }

    pub fn has(&self, i: Instrument) -> bool {
        self.0 & (1 << i as u32) != 0
    }
}
