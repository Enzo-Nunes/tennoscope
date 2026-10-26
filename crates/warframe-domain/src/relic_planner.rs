//! What each relic is worth opened as it is and opened at Radiant, and what the traces between
//! the two buy.
//!
//! Everything here is arithmetic over numbers the caller already holds: the drop table's chances,
//! a value per reward, and how many copies of each refinement the player owns. Nothing is fetched
//! and nothing is guessed. A reward with no value in the chosen currency counts as zero and is
//! counted, so a gap in the price table reads as a gap rather than as a weak relic.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Refinement {
    Intact,
    Exceptional,
    Flawless,
    Radiant,
}

impl Refinement {
    pub const ALL: [Self; 4] = [
        Self::Intact,
        Self::Exceptional,
        Self::Flawless,
        Self::Radiant,
    ];

    pub const fn index(self) -> usize {
        match self {
            Self::Intact => 0,
            Self::Exceptional => 1,
            Self::Flawless => 2,
            Self::Radiant => 3,
        }
    }

    /// What the game charges to refine an Intact relic this far. Refining a refined relic further
    /// costs the difference, so a Flawless copy reaches Radiant for 50.
    pub const fn traces_from_intact(self) -> u32 {
        match self {
            Self::Intact => 0,
            Self::Exceptional => 25,
            Self::Flawless => 50,
            Self::Radiant => 100,
        }
    }

    pub const fn traces_to_radiant(self) -> u32 {
        Self::Radiant.traces_from_intact() - self.traces_from_intact()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Currency {
    Platinum,
    Ducats,
}

/// One of a relic's six rewards, with its chance at each refinement as a fraction.
#[derive(Clone, Debug, PartialEq)]
pub struct PlannedReward {
    pub name: String,
    pub chances: [f64; 4],
    pub platinum: Option<u32>,
    pub ducats: Option<u32>,
    pub owned: u32,
}

impl PlannedReward {
    fn value(&self, currency: Currency) -> Option<u32> {
        match currency {
            Currency::Platinum => self.platinum,
            Currency::Ducats => self.ducats,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlannedRelic {
    /// The relic without its refinement: "Axi A1".
    pub name: String,
    pub vaulted: bool,
    /// Copies held at each refinement, indexed by [`Refinement::index`].
    pub owned: [u32; 4],
    pub rewards: Vec<PlannedReward>,
}

/// The expected value of the best of `squad` independent draws from one drop table.
///
/// A squad cracks one relic each and every player picks from all of the rewards shown, so a shared
/// run is worth the best of the squad's draws to each of them, not one draw. With the rewards
/// sorted by value and `C(k)` the chance of drawing one of the `k` cheapest, the best of `n` draws
/// is at most the `k`th value with chance `C(k)^n`, and the expectation follows by differencing.
///
/// Chances are normalised by their sum. The published tables round: an Intact relic's six chances
/// add to 99.99%, and the last hundredth would otherwise leak out of every squad's expectation.
pub fn expected_best(draws: &[(f64, f64)], squad: u32) -> f64 {
    let total: f64 = draws.iter().map(|(chance, _)| chance).sum();
    if total <= 0.0 || squad == 0 {
        return 0.0;
    }
    let mut sorted = draws.to_vec();
    sorted.sort_by(|left, right| left.1.partial_cmp(&right.1).unwrap_or(Ordering::Equal));
    let squad = i32::try_from(squad).unwrap_or(i32::MAX);
    let mut cumulative = 0.0;
    let mut expected = 0.0;
    for (chance, value) in sorted {
        let below = cumulative / total;
        cumulative += chance;
        let at_or_below = cumulative / total;
        expected += value * (at_or_below.powi(squad) - below.powi(squad));
    }
    expected
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RelicPlanRow {
    pub name: String,
    pub vaulted: bool,
    pub owned: [u32; 4],
    /// What one relic opened at each refinement is worth to you, at the plan's squad size.
    pub value: [f64; 4],
    /// Rewards with no value in the plan's currency, counted as zero above.
    pub unvalued: u32,
    pub rewards: Vec<RewardRow>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RewardRow {
    pub name: String,
    pub chances: [f64; 4],
    pub platinum: Option<u32>,
    pub ducats: Option<u32>,
    pub owned: u32,
}

/// One choice the player can make: copies of a relic held at one refinement, or, for a relic not
/// held at all, a fresh Intact copy.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PlanEntry {
    pub relic: String,
    pub vaulted: bool,
    pub refinement: Refinement,
    /// Copies held at `refinement`; zero for a relic not held.
    pub copies: u32,
    /// What one copy is worth opened as it is, and opened at Radiant.
    pub value: f64,
    pub radiant_value: f64,
    /// Value gained for every 100 Void Traces spent finishing a copy to Radiant. Negative when
    /// refining loses value; `None` for a copy already at Radiant.
    pub gain_per_100: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RelicPlan {
    pub currency: Currency,
    pub squad: u32,
    /// Every relic, in name order, with its value at each refinement and its rewards.
    pub rows: Vec<RelicPlanRow>,
    /// One entry per refinement held, plus one Intact entry per relic not held, in name order.
    pub entries: Vec<PlanEntry>,
}

/// Every relic valued at one currency and squad size.
///
/// The gain is per 100 traces rather than per copy because finishing a part-refined copy costs
/// less: a Flawless copy reaches Radiant for 50, so its gain counts double against an Intact
/// copy's 100. Sorted by it, the top of the list is where traces buy the most value and the bottom
/// is where they buy nothing or lose value, because refining moves chance away from the commons.
pub fn plan(relics: &[PlannedRelic], currency: Currency, squad: u32) -> RelicPlan {
    let squad = squad.clamp(1, 4);
    let mut rows: Vec<RelicPlanRow> = relics
        .iter()
        .map(|relic| plan_row(relic, currency, squad))
        .collect();
    rows.sort_by(|left, right| left.name.cmp(&right.name));

    let entries = rows
        .iter()
        .flat_map(|row| {
            let held = row.owned.iter().any(|&copies| copies > 0);
            Refinement::ALL
                .into_iter()
                .filter(move |refinement| {
                    row.owned[refinement.index()] > 0
                        || (!held && *refinement == Refinement::Intact)
                })
                .map(|refinement| {
                    let value = row.value[refinement.index()];
                    let radiant_value = row.value[Refinement::Radiant.index()];
                    let cost = refinement.traces_to_radiant();
                    PlanEntry {
                        relic: row.name.clone(),
                        vaulted: row.vaulted,
                        refinement,
                        copies: row.owned[refinement.index()],
                        value,
                        radiant_value,
                        gain_per_100: (cost > 0)
                            .then(|| (radiant_value - value) * 100.0 / f64::from(cost)),
                    }
                })
        })
        .collect();

    RelicPlan {
        currency,
        squad,
        rows,
        entries,
    }
}

fn plan_row(relic: &PlannedRelic, currency: Currency, squad: u32) -> RelicPlanRow {
    let value = Refinement::ALL.map(|refinement| {
        let draws: Vec<(f64, f64)> = relic
            .rewards
            .iter()
            .map(|reward| {
                (
                    reward.chances[refinement.index()],
                    f64::from(reward.value(currency).unwrap_or(0)),
                )
            })
            .collect();
        expected_best(&draws, squad)
    });
    RelicPlanRow {
        name: relic.name.clone(),
        vaulted: relic.vaulted,
        owned: relic.owned,
        value,
        unvalued: u32::try_from(
            relic
                .rewards
                .iter()
                .filter(|reward| reward.value(currency).is_none())
                .count(),
        )
        .unwrap_or(u32::MAX),
        rewards: relic
            .rewards
            .iter()
            .map(|reward| RewardRow {
                name: reward.name.clone(),
                chances: reward.chances,
                platinum: reward.platinum,
                ducats: reward.ducats,
                owned: reward.owned,
            })
            .collect(),
    }
}
