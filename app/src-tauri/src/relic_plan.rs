//! The Relics tab: what each relic is worth as it is and at Radiant.
//!
//! The drop tables and ducat values are public reference data, so the planner works in every
//! access mode, Companion included. The game monitor loads its own copies only while it runs,
//! which is never in Companion, so the planner loads them itself, once, the first time the tab
//! asks. A player who never opens the tab never pays for the catalogue parse.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use warframe_acquisition::{
    CatalogCache, PriceTable, RelicCatalogCache, RelicDropTable, WfcdCatalogHttp,
    WfcdRelicCatalogHttp, holding_matches_reward, reward_name_matches,
};
use warframe_domain::{Collection, Currency, PlannedRelic, PlannedReward, RelicPlan, plan};

/// The drop tables and the ducats each reward is worth, joined once per load.
pub struct RelicReference {
    drops: RelicDropTable,
    ducats: HashMap<String, u32>,
}

impl RelicReference {
    /// `catalog` is the WFCD item catalogue's reward entries, `(name, ducats)`. Without it every
    /// reward is unvalued in ducats, which the plan reports, rather than worth zero silently.
    pub fn new(drops: RelicDropTable, catalog: &[(String, u32)]) -> Self {
        let ducats = drops
            .reward_names()
            .into_iter()
            .filter_map(|reward| {
                catalog
                    .iter()
                    .find(|(name, _)| reward_name_matches(name, reward))
                    .map(|(_, ducats)| (reward.to_owned(), *ducats))
            })
            .collect();
        Self { drops, ducats }
    }
}

#[derive(Default)]
enum Load {
    #[default]
    NotStarted,
    Loading,
    Ready(Arc<RelicReference>),
    Failed,
}

#[derive(Default)]
pub struct RelicPlanState(Mutex<Load>);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Loading,
    Ready,
    Unavailable,
}

#[derive(Debug, Serialize)]
pub struct RelicPlanView {
    pub status: PlanStatus,
    pub plan: Option<RelicPlan>,
    /// Whether owned counts come from an inventory snapshot at all.
    pub inventory_known: bool,
}

/// What the tab can have right now.
pub enum Lookup {
    Ready(Arc<RelicReference>),
    /// Nothing is loading: the caller starts the load and reports it as loading.
    Start,
    Loading,
    /// The last load failed. The next lookup starts another, which is the tab's retry; this one
    /// does not, so a tab polling while offline is told so instead of retrying forever.
    Unavailable,
}

impl RelicPlanView {
    pub fn pending(status: PlanStatus) -> Self {
        Self {
            status,
            plan: None,
            inventory_known: false,
        }
    }
}

impl RelicPlanState {
    pub fn lookup(&self) -> Lookup {
        let Ok(mut load) = self.0.lock() else {
            return Lookup::Unavailable;
        };
        match &*load {
            Load::Ready(reference) => Lookup::Ready(Arc::clone(reference)),
            Load::Loading => Lookup::Loading,
            Load::Failed => {
                *load = Load::NotStarted;
                Lookup::Unavailable
            }
            Load::NotStarted => {
                *load = Load::Loading;
                Lookup::Start
            }
        }
    }

    pub fn finish(&self, reference: Option<RelicReference>) {
        if let Ok(mut load) = self.0.lock() {
            *load = match reference {
                Some(reference) => Load::Ready(Arc::new(reference)),
                None => Load::Failed,
            };
        }
    }
}

/// The drop tables and the catalogue's ducats, from the on-disk caches the monitor shares, or
/// from the network on a first run. `None` only when the drop tables cannot be had at all.
pub fn load_reference(app_data: &Path) -> Option<RelicReference> {
    let directory = app_data.join("catalog");
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    let relics = RelicCatalogCache::new(&directory);
    let drops = match WfcdRelicCatalogHttp::new() {
        Ok(source) => relics.load_fresh(&source, now),
        Err(_) => relics.load_cached(),
    }
    .ok()?
    .drops()
    .clone();
    let catalog = CatalogCache::new(&directory);
    let entries = catalog
        .load_cached()
        .ok()
        .or_else(|| {
            WfcdCatalogHttp::new()
                .ok()
                .and_then(|source| catalog.load(&source, now).ok())
        })
        .map(|load| {
            load.index()
                .reward_entries()
                .into_iter()
                .map(|entry| (entry.name, entry.ducats))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Some(RelicReference::new(drops, &entries))
}

/// The planner's inputs: every relic with its held copies, and every reward with its price,
/// ducats and held count.
pub fn planned_relics(
    reference: &RelicReference,
    collection: &Collection,
    prices: Option<&PriceTable>,
) -> Vec<PlannedRelic> {
    let held: HashMap<&str, u32> = collection
        .entries()
        .filter(|entry| entry.quantity > 0)
        .map(|entry| (entry.item.id.as_str(), entry.quantity))
        .collect();
    let mut reward_held: HashMap<&str, u32> = HashMap::new();
    for name in reference.drops.reward_names() {
        let count = collection
            .entries()
            .filter(|entry| holding_matches_reward(&entry.item.name, name))
            .map(|entry| entry.quantity)
            .sum();
        reward_held.insert(name, count);
    }
    reference
        .drops
        .relics()
        .iter()
        .map(|relic| PlannedRelic {
            name: relic.name.clone(),
            vaulted: relic.vaulted,
            owned: relic
                .paths
                .each_ref()
                .map(|path| held.get(path.as_str()).copied().unwrap_or(0)),
            rewards: relic
                .rewards
                .iter()
                .map(|(name, chances)| PlannedReward {
                    name: name.clone(),
                    chances: *chances,
                    platinum: prices.and_then(|prices| prices.price_for(name)),
                    ducats: reference.ducats.get(name).copied(),
                    owned: reward_held.get(name.as_str()).copied().unwrap_or(0),
                })
                .collect(),
        })
        .collect()
}

/// What the tab asks for: the currency and the squad size.
#[derive(Clone, Copy, Debug, Deserialize)]
pub struct PlanSettings {
    pub currency: Currency,
    pub squad: u32,
}

pub fn plan_view(
    reference: &RelicReference,
    collection: &Collection,
    prices: Option<&PriceTable>,
    inventory_known: bool,
    settings: PlanSettings,
) -> RelicPlanView {
    let relics = planned_relics(reference, collection, prices);
    RelicPlanView {
        status: PlanStatus::Ready,
        plan: Some(plan(&relics, settings.currency, settings.squad)),
        inventory_known,
    }
}

#[cfg(test)]
mod tests {
    use warframe_acquisition::{PriceTable, RelicDropTable};
    use warframe_domain::{
        CatalogItem, Category, Collection, Currency, InventoryEntry, InventorySnapshot, ItemId,
    };

    use super::{PlanSettings, RelicReference, plan_view, planned_relics};

    const AXI_A1: &[u8] =
        include_bytes!("../../../crates/warframe-acquisition/tests/fixtures/relics-axi-a1.json");

    fn entry(path: &str, name: &str, category: Category, quantity: u32) -> InventoryEntry {
        InventoryEntry::new(
            CatalogItem::new(ItemId::new(path).unwrap(), name, category).unwrap(),
            quantity,
        )
    }

    fn collection() -> Collection {
        let mut collection = Collection::default();
        collection.replace(
            InventorySnapshot::coherent(vec![
                entry(
                    "/Lotus/Types/Game/Projections/T4VoidProjectionEBronze",
                    "Axi A1 Intact",
                    Category::Relic,
                    3,
                ),
                entry(
                    "/Lotus/Types/Game/Projections/T4VoidProjectionEPlatinum",
                    "Axi A1 Radiant",
                    Category::Relic,
                    1,
                ),
                entry(
                    "/Lotus/Types/Recipes/Weapons/WeaponParts/BratonPrimeStock",
                    "Braton Prime Stock",
                    Category::PrimePart,
                    2,
                ),
            ])
            .unwrap(),
        );
        collection
    }

    fn reference() -> RelicReference {
        RelicReference::new(
            RelicDropTable::from_wfcd_json(AXI_A1).unwrap(),
            &[
                ("Nikana Prime Blueprint".to_owned(), 100),
                ("Braton Prime Stock".to_owned(), 15),
            ],
        )
    }

    #[test]
    fn held_relics_are_counted_per_refinement_by_inventory_path() {
        let relics = planned_relics(&reference(), &collection(), None);
        assert_eq!(relics[0].name, "Axi A1");
        assert_eq!(relics[0].owned, [3, 0, 0, 1]);
    }

    #[test]
    fn rewards_carry_their_price_ducats_and_held_count() {
        let prices = PriceTable::from_dump_json(
            br#"{"Nikana Prime Blueprint":[{"order_type":"closed","median":40.0,"volume":5}]}"#,
            "2026-10-25",
        )
        .unwrap();
        let relics = planned_relics(&reference(), &collection(), Some(&prices));
        let reward = |name: &str| {
            relics[0]
                .rewards
                .iter()
                .find(|reward| reward.name == name)
                .unwrap()
                .clone()
        };
        assert_eq!(reward("Nikana Prime Blueprint").platinum, Some(40));
        assert_eq!(reward("Nikana Prime Blueprint").ducats, Some(100));
        assert_eq!(reward("Braton Prime Stock").owned, 2);
        assert_eq!(reward("Braton Prime Stock").platinum, None);
        assert_eq!(reward("Fragor Prime Head").ducats, None);
    }

    #[test]
    fn the_view_lists_each_held_refinement() {
        let view = plan_view(
            &reference(),
            &collection(),
            None,
            true,
            PlanSettings {
                currency: Currency::Ducats,
                squad: 1,
            },
        );
        let plan = view.plan.unwrap();
        assert_eq!(plan.rows.len(), 1);
        assert_eq!(plan.rows[0].unvalued, 4);
        assert_eq!(plan.entries.len(), 2);
    }
}
