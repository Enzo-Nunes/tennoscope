#![forbid(unsafe_code)]

mod catalog;
mod error;
mod inventory;
mod mastery;
mod relic_planner;
mod rewards;

pub use catalog::{CatalogItem, Category, ItemId};
pub use error::DomainError;
pub use inventory::{Collection, InventoryEntry, InventorySnapshot};
pub use mastery::{KioskMastery, MasteryMark, SetPart};
pub use relic_planner::{
    Currency, PlanEntry, PlannedRelic, PlannedReward, Refinement, RelicPlan, RelicPlanRow,
    RewardRow, expected_best, plan,
};
pub use rewards::{RewardAdvisor, RewardCandidate, RewardView};
