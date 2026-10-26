use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use crate::{CatalogError, CatalogIndex, RewardNeedle};

#[derive(Clone, Debug, Default)]
pub struct RelicRewardIndex {
    rewards: BTreeMap<String, BTreeSet<String>>,
}

impl RelicRewardIndex {
    pub fn from_wfcd_json(bytes: &[u8]) -> Result<Self, CatalogError> {
        let records: Vec<RelicRecord> =
            serde_json::from_slice(bytes).map_err(|_| CatalogError::InvalidJson)?;
        let mut rewards = BTreeMap::new();
        for record in records {
            if !record
                .unique_name
                .starts_with("/Lotus/Types/Game/Projections/")
            {
                continue;
            }
            let names = record
                .rewards
                .into_iter()
                .map(|reward| reward.item.name.trim().to_owned())
                .filter(|name| !name.is_empty())
                .collect::<BTreeSet<_>>();
            if !names.is_empty() {
                rewards.insert(record.unique_name, names);
            }
        }
        Ok(Self { rewards })
    }

    /// Every reward a relic in the index can drop, spelled as the relic tables and the reward
    /// screen spell it: "2X Forma Blueprint", "Lavos Prime Chassis Blueprint". A reward several
    /// relics share comes once per relic.
    pub fn reward_names(&self) -> impl Iterator<Item = &str> {
        self.rewards.values().flatten().map(String::as_str)
    }

    pub fn candidates_for_projection_paths(
        &self,
        projection_paths: &[String],
        catalog: &CatalogIndex,
    ) -> Vec<RewardNeedle> {
        projection_paths
            .iter()
            .filter_map(|path| self.rewards.get(path))
            .flatten()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter_map(|name| {
                RewardNeedle::from_paths(name.clone(), reward_catalog_paths(name, catalog)).ok()
            })
            .collect()
    }
}

fn reward_catalog_paths(name: &str, catalog: &CatalogIndex) -> Vec<String> {
    // Relic rewards conventionally add ` Blueprint` to component names. Prefer the component
    // identity even when the catalog also has an exact inventory-recipe alias for that wording.
    if let Some(component_name) = name.strip_suffix(" Blueprint") {
        let paths = catalog.paths_for_name(component_name);
        if !paths.is_empty() {
            return paths;
        }
    }
    let exact = catalog.paths_for_name(name);
    if !exact.is_empty() {
        return exact;
    }

    for alias in [
        crate::catalog::without_quantity(name),
        name.strip_suffix(" Blueprint"),
    ]
    .into_iter()
    .flatten()
    {
        let paths = catalog.paths_for_name(alias);
        if !paths.is_empty() {
            return paths;
        }
        if let Some(component_name) = alias.strip_suffix(" Blueprint") {
            let paths = catalog.paths_for_name(component_name);
            if !paths.is_empty() {
                return paths;
            }
        }
    }
    Vec::new()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RelicRecord {
    unique_name: String,
    #[serde(default)]
    rewards: Vec<RelicReward>,
}

#[derive(Deserialize)]
struct RelicReward {
    item: RelicRewardItem,
}

#[derive(Deserialize)]
struct RelicRewardItem {
    name: String,
}

/// One relic's drop table at every refinement, for the planner.
#[derive(Clone, Debug, PartialEq)]
pub struct RelicDrops {
    /// The relic without its refinement: "Axi A1".
    pub name: String,
    pub vaulted: bool,
    /// The inventory path of each refinement, indexed by `Refinement::index`. The game holds each
    /// refinement as its own item, so this is how held copies are counted.
    pub paths: [String; 4],
    /// The six rewards, each with its chance as a fraction at every refinement.
    pub rewards: Vec<(String, [f64; 4])>,
}

/// Every relic the planner can rank, in name order.
///
/// Built from the same `Relics.json` as [`RelicRewardIndex`], which keeps only names because the
/// reward screen needs nothing more. A relic is kept only when all four refinements are present
/// with the same rewards and chances that add up; anything else would be ranked on a table the
/// game does not use, so it is dropped rather than patched.
#[derive(Clone, Debug, Default)]
pub struct RelicDropTable {
    relics: Vec<RelicDrops>,
}

/// The refinements as WFCD names them, in `Refinement::index` order.
const REFINEMENT_NAMES: [&str; 4] = ["Intact", "Exceptional", "Flawless", "Radiant"];

impl RelicDropTable {
    pub fn from_wfcd_json(bytes: &[u8]) -> Result<Self, CatalogError> {
        let records: Vec<DropRecord> =
            serde_json::from_slice(bytes).map_err(|_| CatalogError::InvalidJson)?;
        let mut grouped: BTreeMap<String, [Option<DropRecord>; 4]> = BTreeMap::new();
        for record in records {
            if !record
                .unique_name
                .starts_with("/Lotus/Types/Game/Projections/")
            {
                continue;
            }
            let Some((base, index)) =
                REFINEMENT_NAMES
                    .iter()
                    .enumerate()
                    .find_map(|(index, word)| {
                        Some((record.name.strip_suffix(word)?.trim_end().to_owned(), index))
                    })
            else {
                continue;
            };
            grouped.entry(base).or_default()[index] = Some(record);
        }
        let relics = grouped
            .into_iter()
            .filter_map(|(name, records)| relic_drops(name, records))
            .collect();
        Ok(Self { relics })
    }

    pub fn relics(&self) -> &[RelicDrops] {
        &self.relics
    }

    /// Every reward name any relic drops, once each.
    pub fn reward_names(&self) -> BTreeSet<&str> {
        self.relics
            .iter()
            .flat_map(|relic| relic.rewards.iter().map(|(name, _)| name.as_str()))
            .collect()
    }
}

fn relic_drops(name: String, records: [Option<DropRecord>; 4]) -> Option<RelicDrops> {
    let [intact, exceptional, flawless, radiant] = records;
    let records = [intact?, exceptional?, flawless?, radiant?];
    let names: Vec<String> = records[0]
        .rewards
        .iter()
        .map(|reward| reward.item.name.trim().to_owned())
        .collect();
    if names.is_empty() || names.iter().any(String::is_empty) {
        return None;
    }
    let mut rewards: Vec<(String, [f64; 4])> =
        names.into_iter().map(|name| (name, [0.0; 4])).collect();
    for (index, record) in records.iter().enumerate() {
        if record.rewards.len() != rewards.len() {
            return None;
        }
        // Matched by position, not by name: a relic can list one reward twice at two rarities
        // (`Meso D1` drops "2X Forma Blueprint" as a common and as an uncommon). WFCD lists every
        // refinement's rewards in the same order, and a table that does not is dropped.
        for (slot, reward) in rewards.iter_mut().zip(&record.rewards) {
            if slot.0 != reward.item.name.trim()
                || !reward.chance.is_finite()
                || reward.chance <= 0.0
            {
                return None;
            }
            slot.1[index] = reward.chance / 100.0;
        }
        // The tables round to hundredths of a percent: an Intact relic adds to 99.99.
        let total: f64 = rewards.iter().map(|(_, chances)| chances[index]).sum();
        if (total - 1.0).abs() > 0.001 {
            return None;
        }
    }
    let vaulted = records[0].vaulted;
    let paths = records.map(|record| record.unique_name);
    Some(RelicDrops {
        name,
        vaulted,
        paths,
        rewards,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DropRecord {
    unique_name: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    vaulted: bool,
    #[serde(default)]
    rewards: Vec<DropReward>,
}

#[derive(Deserialize)]
struct DropReward {
    #[serde(default)]
    chance: f64,
    item: RelicRewardItem,
}
